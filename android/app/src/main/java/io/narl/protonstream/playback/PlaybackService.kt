package io.narl.protonstream.playback

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.graphics.Bitmap
import android.graphics.drawable.Icon
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.media.MediaMetadata
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.Binder
import android.os.IBinder
import androidx.core.content.ContextCompat
import io.narl.protonstream.MainActivity
import io.narl.protonstream.R
import io.narl.protonstream.settings.SettingsStore
import io.narl.protonstream.ui.loadArtwork
import io.narl.protonstream.ui.loadThumbnail
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

/** Owns libmpv so audio survives Activity recreation, rotation and PiP. */
class PlaybackService : Service() {
    inner class PlaybackBinder : Binder() {
        val host: NativeMpvHost? get() = this@PlaybackService.host
    }

    /**
     * The libmpv core, or null when this build could not start one.
     *
     * Nullable rather than a `lateinit` that throws: the system creates this
     * service, so a failure in its constructor takes the process down and makes
     * the app's own "playback is unavailable" screen unreachable.
     */
    private var host: NativeMpvHost? = null
    private lateinit var mediaSession: MediaSession
    private lateinit var audioManager: AudioManager
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var audioFocus: AudioFocusRequest? = null
    /** Whether the pause on screen is this service's doing, not the viewer's. */
    private var pausedForFocus = false
    /** The volume before ducking, so the restore does not read a ducked one. */
    private var beforeDucking: Double? = null
    /**
     * Pulling the headphone jack must not continue on the speaker.
     *
     * Android broadcasts this and expects every media app to handle it; the
     * alternative is an episode playing out loud in a quiet room.
     */
    private val becomingNoisy = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
            if (intent?.action == AudioManager.ACTION_AUDIO_BECOMING_NOISY) host?.setPaused(true)
        }
    }
    private var foregroundPlayback = false
    private var sessionPlayback = false
    /**
     * What the posted notification already says.
     *
     * The state poll runs four times a second and almost none of those readings
     * change anything the shade shows. Re-posting on every one of them animates
     * the media control and costs a binder round trip per tick, so the
     * notification is rebuilt only when its own inputs move.
     */
    private var notified: NotificationShape? = null
    /** The pending teardown, held so an episode transition can call it off. */
    private var retire: Job? = null
    /** The episode's artwork, and the source key it was fetched for. */
    private var artwork: Pair<String, Bitmap>? = null
    private val binder = PlaybackBinder()

    override fun onCreate() {
        super.onCreate()
        createChannel()
        ContextCompat.registerReceiver(
            this,
            becomingNoisy,
            IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY),
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        audioManager = getSystemService(AudioManager::class.java)
        host = NativeMpvHost.createOrNull()?.apply {
            onPlaybackStarted = {
                // A refusal is rare — a call in progress — but playing over it
                // is worse than not playing, and the viewer can simply press
                // play again once the call ends.
                if (!requestAudioFocus()) setPaused(true)
                sessionPlayback = true
                cancelRetire()
                if (!foregroundPlayback && SettingsStore(this@PlaybackService).backgroundAudio) {
                    // Convert the bound service into a started foreground
                    // service only when the viewer opted to keep audio alive.
                    // Already-foreground is the ordinary case across an episode
                    // transition, and re-entering costs a notification rebuild.
                    foregroundPlayback = runCatching {
                        startService(Intent(this@PlaybackService, PlaybackService::class.java))
                        startForeground(NOTIFICATION_ID, notification(state.value, nowPlaying.value))
                    }.isSuccess
                }
                mediaSession.isActive = true
            }
            onStateChanged = { updateSession(it) }
            onExplicitStop = { retirePlayback() }
        }
        mediaSession = MediaSession(this, "proton-stream").apply {
            setSessionActivity(openActivity())
            setCallback(object : MediaSession.Callback() {
                override fun onPlay() { host?.setPaused(false) }
                override fun onPause() { host?.setPaused(true) }
                override fun onSeekTo(pos: Long) { host?.seek(pos / 1_000.0) }
                override fun onStop() { host?.stop() }
                override fun onSkipToNext() { host?.onSkipNext?.invoke() }
                override fun onSkipToPrevious() { host?.onSkipPrevious?.invoke() }
            })
        }
        // Artwork arrives after the episode does, and the shape of the transport
        // changes with the playlist position, so the notification follows the
        // published episode rather than being built once when playback starts.
        val core = host ?: return
        scope.launch {
            core.nowPlaying.collect { playing ->
                // Keyed by whichever source the episode has: with metadata off
                // there is no provider poster, and Proton's own thumbnail is
                // the only thing the shade can show.
                artwork = playing?.let { episode ->
                    val key = episode.artworkUrl ?: episode.artworkFile?.key ?: return@let null
                    artwork?.takeIf { it.first == key }
                        ?: (
                            loadArtwork(this@PlaybackService, episode.artworkUrl)
                                ?: loadThumbnail(episode.artworkFile)
                            )?.let { key to it }
                }
                refreshSession(core.state.value, playing)
            }
        }
    }

    override fun onBind(intent: Intent?): IBinder = binder

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_PLAY -> host?.setPaused(false)
            ACTION_PAUSE -> host?.setPaused(true)
            ACTION_PREVIOUS -> host?.onSkipPrevious?.invoke()
            ACTION_NEXT -> host?.onSkipNext?.invoke()
            ACTION_STOP -> host?.stop()
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        scope.cancel()
        runCatching { unregisterReceiver(becomingNoisy) }
        audioFocus?.let(audioManager::abandonAudioFocusRequest)
        mediaSession.release()
        host?.close()
        super.onDestroy()
    }

    /**
     * Take audio focus, and honour what the system says about it afterwards.
     *
     * The four cases are genuinely different and collapsing them is what made
     * this feel broken: a phone call is transient and playback should come back
     * by itself; a navigation prompt wants the volume down, not a pause; another
     * media app taking over for good means giving the focus back rather than
     * holding it; and a refusal means not playing at all.
     */
    private fun requestAudioFocus(): Boolean {
        audioFocus?.let(audioManager::abandonAudioFocusRequest)
        val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
            .setAudioAttributes(
                AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MOVIE).build(),
            )
            .setWillPauseWhenDucked(false)
            .setOnAudioFocusChangeListener { focus ->
                when (focus) {
                    AudioManager.AUDIOFOCUS_LOSS -> {
                        pausedForFocus = false
                        audioFocus?.let(audioManager::abandonAudioFocusRequest)
                        audioFocus = null
                        host?.setPaused(true)
                    }
                    AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> {
                        // Only resume what this paused: a viewer who paused
                        // before the phone rang expects it still paused after.
                        pausedForFocus = host?.state?.value?.paused == false
                        host?.setPaused(true)
                    }
                    AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK -> {
                        host?.let { core ->
                            val current = core.state.value.volume
                            beforeDucking = beforeDucking ?: current
                            core.setVolume(current * DUCK_FACTOR)
                        }
                    }
                    AudioManager.AUDIOFOCUS_GAIN -> {
                        beforeDucking?.let { host?.setVolume(it) }
                        beforeDucking = null
                        if (pausedForFocus) {
                            pausedForFocus = false
                            host?.setPaused(false)
                        }
                    }
                }
            }.build()
        audioFocus = request
        return audioManager.requestAudioFocus(request) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
    }

    private fun updateSession(state: MpvPlaybackState) {
        refreshSession(state, host?.nowPlaying?.value)
        if (state.ended) scheduleRetire() else cancelRetire()
    }

    /**
     * Retire the session once playback is really over.
     *
     * "This file ended" is not "playback is over": autoplay walks to the next
     * episode, and opening its stream is a network round trip. Retiring inside
     * that gap leaves the app with no foreground state at all, so the
     * `startForeground` on the far side is a background start — which API 31+
     * refuses with `ForegroundServiceStartNotAllowedException`, and this app's
     * minSdk is 31.
     */
    private fun scheduleRetire() {
        if (retire?.isActive == true) return
        retire = scope.launch {
            // The end of file reaches the player and this service from the same
            // poll, and the player's advance is a recomposition behind. Give it
            // that long to declare a transition before concluding there is none.
            delay(TRANSITION_SETTLE_MS)
            val host = this@PlaybackService.host ?: return@launch
            withTimeoutOrNull(TRANSITION_GRACE_MS) { host.advancing.first { !it } }
            if (host.state.value.ended) retirePlayback()
        }
    }

    private fun cancelRetire() {
        retire?.cancel()
        retire = null
    }

    private fun refreshSession(state: MpvPlaybackState, playing: NowPlaying?) {
        val status = when {
            state.ended -> PlaybackState.STATE_STOPPED
            state.paused -> PlaybackState.STATE_PAUSED
            else -> PlaybackState.STATE_PLAYING
        }
        var actions = PlaybackState.ACTION_PLAY or PlaybackState.ACTION_PAUSE or
            PlaybackState.ACTION_PLAY_PAUSE or PlaybackState.ACTION_SEEK_TO or PlaybackState.ACTION_STOP
        if (playing?.hasNext == true) actions = actions or PlaybackState.ACTION_SKIP_TO_NEXT
        if (playing?.hasPrevious == true) actions = actions or PlaybackState.ACTION_SKIP_TO_PREVIOUS
        mediaSession.setPlaybackState(
            PlaybackState.Builder()
                .setActions(actions)
                .setState(status, (state.position * 1_000).toLong(), if (state.paused) 0f else 1f)
                .build(),
        )
        mediaSession.setMetadata(metadata(state, playing))
        val shape = NotificationShape(
            paused = state.paused,
            media = playing?.media,
            title = playing?.title,
            subtitle = playing?.show,
            hasPrevious = playing?.hasPrevious == true,
            hasNext = playing?.hasNext == true,
            hasArtwork = artwork != null,
        )
        if (foregroundPlayback && notified != shape) {
            notified = shape
            getSystemService(NotificationManager::class.java)
                .notify(NOTIFICATION_ID, notification(state, playing))
        }
    }

    /**
     * What the shade, the lock screen and Android Auto read.
     *
     * The episode is the title and the show is the artist, which is the mapping
     * every media surface already lays out well: one prominent line, one
     * secondary, and the poster behind both.
     */
    private fun metadata(state: MpvPlaybackState, playing: NowPlaying?): MediaMetadata =
        MediaMetadata.Builder()
            .putString(MediaMetadata.METADATA_KEY_TITLE, playing?.title ?: "proton-stream")
            .putString(MediaMetadata.METADATA_KEY_DISPLAY_TITLE, playing?.title ?: "proton-stream")
            .apply {
                playing?.show?.let {
                    putString(MediaMetadata.METADATA_KEY_ARTIST, it)
                    putString(MediaMetadata.METADATA_KEY_ALBUM, it)
                    putString(MediaMetadata.METADATA_KEY_DISPLAY_SUBTITLE, it)
                }
                playing?.detail?.let {
                    putString(MediaMetadata.METADATA_KEY_DISPLAY_DESCRIPTION, it)
                }
                artwork?.second?.let {
                    putBitmap(MediaMetadata.METADATA_KEY_ALBUM_ART, it)
                    putBitmap(MediaMetadata.METADATA_KEY_ART, it)
                    putBitmap(MediaMetadata.METADATA_KEY_DISPLAY_ICON, it)
                }
            }
            .putLong(MediaMetadata.METADATA_KEY_DURATION, (state.duration * 1_000).toLong())
            .build()

    private fun retirePlayback() {
        cancelRetire()
        if (!sessionPlayback && !foregroundPlayback) return
        sessionPlayback = false
        notified = null
        artwork = null
        mediaSession.isActive = false
        audioFocus?.let(audioManager::abandonAudioFocusRequest)
        audioFocus = null
        if (foregroundPlayback) stopForeground(STOP_FOREGROUND_REMOVE)
        foregroundPlayback = false
        stopSelf()
    }

    private fun openActivity(): PendingIntent = PendingIntent.getActivity(
        this,
        0,
        Intent(this, MainActivity::class.java),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )

    private fun command(requestCode: Int, action: String): PendingIntent = PendingIntent.getService(
        this,
        requestCode,
        Intent(this, PlaybackService::class.java).setAction(action),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )

    private fun action(icon: Int, label: String, requestCode: Int, name: String): Notification.Action =
        Notification.Action.Builder(Icon.createWithResource(this, icon), label, command(requestCode, name)).build()

    private fun notification(state: MpvPlaybackState, playing: NowPlaying?): Notification {
        val paused = state.paused
        val actions = buildList {
            if (playing?.hasPrevious == true) {
                add(action(android.R.drawable.ic_media_previous, "Previous", 3, ACTION_PREVIOUS))
            }
            add(
                if (paused) action(android.R.drawable.ic_media_play, "Play", 1, ACTION_PLAY)
                else action(android.R.drawable.ic_media_pause, "Pause", 1, ACTION_PAUSE),
            )
            if (playing?.hasNext == true) {
                add(action(android.R.drawable.ic_media_next, "Next", 4, ACTION_NEXT))
            }
            add(action(android.R.drawable.ic_menu_close_clear_cancel, "Stop", 2, ACTION_STOP))
        }
        val style = Notification.MediaStyle()
            .setMediaSession(mediaSession.sessionToken)
            // The compact view holds three: the transport as it exists for this
            // episode, so a first episode does not show a dead Previous.
            .setShowActionsInCompactView(*IntArray(minOf(3, actions.size)) { it })
        return Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_notification_stream)
            .setContentTitle(playing?.title ?: "proton-stream")
            .setContentText(playing?.show ?: playing?.detail ?: if (paused) "Paused" else "Playing")
            .apply {
                playing?.takeIf { it.show != null }?.detail?.let { setSubText(it) }
                artwork?.second?.let { setLargeIcon(it) }
            }
            .setColor(ACCENT)
            .setColorized(true)
            .setContentIntent(openActivity())
            .setDeleteIntent(command(2, ACTION_STOP))
            .setVisibility(Notification.VISIBILITY_PUBLIC)
            .setOngoing(!paused)
            .apply { actions.forEach { addAction(it) } }
            .setStyle(style)
            .build()
    }

    private fun createChannel() {
        getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL_ID, "Playback", NotificationManager.IMPORTANCE_LOW).apply {
                description = "Transport controls for the episode being played."
                setShowBadge(false)
            },
        )
    }

    /** The inputs the posted notification is built from, and nothing else. */
    private data class NotificationShape(
        val paused: Boolean,
        val media: String?,
        val title: String?,
        val subtitle: String?,
        val hasPrevious: Boolean,
        val hasNext: Boolean,
        val hasArtwork: Boolean,
    )

    companion object {
        private const val CHANNEL_ID = "playback"
        private const val NOTIFICATION_ID = 47
        /** How long a player gets to declare that it is changing episodes. */
        private const val TRANSITION_SETTLE_MS = 1_000L
        /** And how long that transition may then take before it is abandoned. */
        private const val TRANSITION_GRACE_MS = 30_000L
        /** How far the volume drops for a navigation prompt or a notification. */
        private const val DUCK_FACTOR = 0.3

        /** Catppuccin Mocha mauve, the accent the app is themed with. */
        private const val ACCENT = 0xFFCBA6F7.toInt()
        private const val ACTION_PLAY = "io.narl.protonstream.PLAY"
        private const val ACTION_PAUSE = "io.narl.protonstream.PAUSE"
        private const val ACTION_PREVIOUS = "io.narl.protonstream.PREVIOUS"
        private const val ACTION_NEXT = "io.narl.protonstream.NEXT"
        private const val ACTION_STOP = "io.narl.protonstream.STOP"
    }
}
