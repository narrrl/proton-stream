package io.narl.protonstream.playback

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.media.MediaMetadata
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.Binder
import android.os.IBinder
import io.narl.protonstream.MainActivity
import io.narl.protonstream.settings.SettingsStore

/** Owns libmpv so audio survives Activity recreation, rotation and PiP. */
class PlaybackService : Service() {
    inner class PlaybackBinder : Binder() {
        val host: NativeMpvHost get() = this@PlaybackService.host
    }

    private lateinit var host: NativeMpvHost
    private lateinit var mediaSession: MediaSession
    private lateinit var audioManager: AudioManager
    private var audioFocus: AudioFocusRequest? = null
    private var notifiedPaused: Boolean? = null
    private var foregroundPlayback = false
    private var sessionPlayback = false
    private val binder = PlaybackBinder()

    override fun onCreate() {
        super.onCreate()
        createChannel()
        audioManager = getSystemService(AudioManager::class.java)
        host = NativeMpvHost().apply {
            onPlaybackStarted = {
                requestAudioFocus()
                sessionPlayback = true
                if (SettingsStore(this@PlaybackService).backgroundAudio) {
                    // Convert the bound service into a started foreground
                    // service only when the viewer opted to keep audio alive.
                    startService(Intent(this@PlaybackService, PlaybackService::class.java))
                    startForeground(NOTIFICATION_ID, notification(false))
                    foregroundPlayback = true
                }
                mediaSession.isActive = true
            }
            onStateChanged = { updateSession(it) }
            onExplicitStop = { retirePlayback() }
        }
        mediaSession = MediaSession(this, "proton-stream").apply {
            setCallback(object : MediaSession.Callback() {
                override fun onPlay() = host.setPaused(false)
                override fun onPause() = host.setPaused(true)
                override fun onSeekTo(pos: Long) = host.seek(pos / 1_000.0)
                override fun onStop() = host.stop()
            })
        }
    }

    override fun onBind(intent: Intent?): IBinder = binder

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_PLAY -> host.setPaused(false)
            ACTION_PAUSE -> host.setPaused(true)
            ACTION_STOP -> host.stop()
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        audioFocus?.let(audioManager::abandonAudioFocusRequest)
        mediaSession.release()
        host.close()
        super.onDestroy()
    }

    private fun requestAudioFocus() {
        audioFocus?.let(audioManager::abandonAudioFocusRequest)
        val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
            .setAudioAttributes(
                AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MOVIE).build(),
            )
            .setOnAudioFocusChangeListener { focus ->
                when (focus) {
                    AudioManager.AUDIOFOCUS_LOSS, AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> host.setPaused(true)
                    AudioManager.AUDIOFOCUS_GAIN -> Unit
                }
            }.build()
        audioFocus = request
        audioManager.requestAudioFocus(request)
    }

    private fun updateSession(state: MpvPlaybackState) {
        val status = when {
            state.ended -> PlaybackState.STATE_STOPPED
            state.paused -> PlaybackState.STATE_PAUSED
            else -> PlaybackState.STATE_PLAYING
        }
        mediaSession.setPlaybackState(
            PlaybackState.Builder()
                .setActions(PlaybackState.ACTION_PLAY or PlaybackState.ACTION_PAUSE or PlaybackState.ACTION_SEEK_TO or PlaybackState.ACTION_STOP)
                .setState(status, (state.position * 1_000).toLong(), if (state.paused) 0f else 1f)
                .build(),
        )
        mediaSession.setMetadata(
            MediaMetadata.Builder().putLong(MediaMetadata.METADATA_KEY_DURATION, (state.duration * 1_000).toLong()).build(),
        )
        if (state.ended && foregroundPlayback) {
            host.releaseEndedStream()
            retirePlayback()
            return
        }
        if (foregroundPlayback && notifiedPaused != state.paused) {
            notifiedPaused = state.paused
            getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, notification(state.paused))
        }
    }

    private fun retirePlayback() {
        if (!sessionPlayback && !foregroundPlayback) return
        sessionPlayback = false
        notifiedPaused = null
        mediaSession.isActive = false
        audioFocus?.let(audioManager::abandonAudioFocusRequest)
        audioFocus = null
        if (foregroundPlayback) stopForeground(STOP_FOREGROUND_REMOVE)
        foregroundPlayback = false
        stopSelf()
    }

    private fun notification(paused: Boolean): Notification {
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val toggleAction = if (paused) ACTION_PLAY else ACTION_PAUSE
        val toggle = PendingIntent.getService(
            this, 1, Intent(this, PlaybackService::class.java).setAction(toggleAction), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val stop = PendingIntent.getService(
            this, 2, Intent(this, PlaybackService::class.java).setAction(ACTION_STOP), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(android.R.drawable.ic_media_play)
            .setContentTitle("proton-stream")
            .setContentText(if (paused) "Playback paused" else "Playing")
            .setContentIntent(open)
            .setOngoing(!paused)
            .addAction(Notification.Action.Builder(null, if (paused) "Play" else "Pause", toggle).build())
            .addAction(Notification.Action.Builder(null, "Stop", stop).build())
            .setStyle(Notification.MediaStyle().setMediaSession(mediaSession.sessionToken).setShowActionsInCompactView(0, 1))
            .build()
    }

    private fun createChannel() {
        getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL_ID, "Playback", NotificationManager.IMPORTANCE_LOW),
        )
    }

    companion object {
        private const val CHANNEL_ID = "playback"
        private const val NOTIFICATION_ID = 47
        private const val ACTION_PLAY = "io.narl.protonstream.PLAY"
        private const val ACTION_PAUSE = "io.narl.protonstream.PAUSE"
        private const val ACTION_STOP = "io.narl.protonstream.STOP"
    }
}
