package io.narl.protonstream.playback

import android.view.Surface
import io.narl.protonstream.ui.ThumbnailSource
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock
import uniffi.pstr_android.ChapterPlan
import uniffi.pstr_android.ChapterRecord
import uniffi.pstr_android.chapterPlan

/**
 * Why the open file ended, mirroring `pstr_player::EndReason` so both clients
 * agree on what "finished" means.
 *
 * Only [Eof] is an episode watched to the end. Treating the others as one — a
 * stop, a shutdown, a failed demux — is what marks an episode that never played
 * as watched and autoplays past it.
 */
enum class EndReason {
    None,
    Eof,
    Stopped,
    Quit,
    Failed,
    Other,
    ;

    companion object {
        fun of(value: Double): EndReason = entries.getOrElse(value.toInt()) { Other }
    }
}

data class MpvPlaybackState(
    val position: Double = 0.0,
    val duration: Double = 0.0,
    val volume: Double = 100.0,
    val paused: Boolean = true,
    val muted: Boolean = false,
    val ended: Boolean = false,
    /** Meaningful only while [ended]; [EndReason.None] otherwise. */
    val endReason: EndReason = EndReason.None,
    /**
     * Which episode this reading belongs to, or null before the first poll of a
     * newly loaded file. Everything that *writes* per-episode state — resume
     * position above all — must check this: one host serves every episode, and
     * a reading attributed to the wrong one resumes the next episode where the
     * last one stopped.
     */
    val media: String? = null,
    /** The picture's display size, or zero until mpv has decoded enough to know. */
    val videoWidth: Double = 0.0,
    val videoHeight: Double = 0.0,
    /**
     * mpv has run out of buffered data and stopped to refill.
     *
     * Not the same as [paused], which is the viewer's doing. A frozen picture
     * that nothing explains is indistinguishable from a hung app, which is what
     * a network-backed stream looked like before this existed.
     */
    val buffering: Boolean = false,
    /** How full the demuxer cache is while [buffering], 0–100. */
    val cachePercent: Double = 0.0,
    /** A seek has been issued but playback has not resumed at the new position. */
    val seeking: Boolean = false,
    /** How far the demuxer has read ahead, in seconds into the file. */
    val cachedUntil: Double = 0.0,
) {
    /** Whether the picture is stopped for a reason the viewer did not choose. */
    val stalled: Boolean get() = (buffering || seeking) && !paused && !ended
}

/**
 * What is on screen, for the surfaces that are not the player.
 *
 * The media session, its notification and Picture-in-Picture all have to name
 * the episode, and none of them can reach the composition that knows it. The
 * player publishes this once per episode and they read it.
 */
data class NowPlaying(
    val media: String,
    val title: String,
    val show: String?,
    val detail: String?,
    val artworkUrl: String?,
    /** The file to pull a Proton thumbnail from when the provider has no poster. */
    val artworkFile: ThumbnailSource?,
    val hasPrevious: Boolean,
    val hasNext: Boolean,
)

data class MpvTrack(
    val id: Long,
    val type: String,
    val language: String,
    val title: String,
    val selected: Boolean,
) {
    val label: String get() = listOf(language, title).filter(String::isNotBlank)
        .joinToString(" — ").ifBlank { "${type.replaceFirstChar(Char::uppercase)} $id" }
}

/** Process-local owner of the libmpv core. PlaybackService owns its lifetime. */
class NativeMpvHost private constructor(private val nativeHandle: Long) : LibmpvHost, AutoCloseable {
    private val lifecycle = ReentrantLock()
    @Volatile private var closed = false
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val mutableState = MutableStateFlow(MpvPlaybackState())
    private val mutableTracks = MutableStateFlow(emptyList<MpvTrack>())
    private val mutableChapters = MutableStateFlow(ChapterPlan(emptyList(), null))
    private val mutableNowPlaying = MutableStateFlow<NowPlaying?>(null)
    private val mutableAdvancing = MutableStateFlow(false)
    private val mutableProblem = MutableStateFlow<String?>(null)
    private var poller: Job? = null
    /** The episode key and load generation the native player currently holds. */
    @Volatile private var currentMedia: String? = null
    @Volatile private var currentGeneration: Double = -1.0
    @Volatile private var plannedFor: Pair<Double, Long>? = null
    var onPlaybackStarted: (() -> Unit)? = null
    var onStateChanged: ((MpvPlaybackState) -> Unit)? = null
    var onExplicitStop: (() -> Unit)? = null
    /**
     * Where a transport outside the player sends previous/next.
     *
     * The player owns which episode is open, so the notification, the lock
     * screen and Picture-in-Picture cannot walk the playlist themselves. Both
     * run on the main thread, which is where the player's state lives.
     */
    var onSkipPrevious: (() -> Unit)? = null
    var onSkipNext: (() -> Unit)? = null

    val state: StateFlow<MpvPlaybackState> = mutableState.asStateFlow()
    val tracks: StateFlow<List<MpvTrack>> = mutableTracks.asStateFlow()
    /** The open file's chapters, already resolved into openings and endings. */
    val chapters: StateFlow<ChapterPlan> = mutableChapters.asStateFlow()
    /** The episode the player has open, for every surface that has to name it. */
    val nowPlaying: StateFlow<NowPlaying?> = mutableNowPlaying.asStateFlow()
    /**
     * True while the player is between episodes.
     *
     * The end of one episode and the start of the next are separated by a
     * network round trip — opening the next revision's stream. The foreground
     * service must not retire in that gap: with no foreground state left, the
     * `startForeground` on the other side is a background start, which API 31+
     * refuses outright.
     */
    val advancing: StateFlow<Boolean> = mutableAdvancing.asStateFlow()
    /**
     * What mpv last complained about, until the viewer dismisses it.
     *
     * Errors mpv reports are about the *file* — an unreadable header, a codec it
     * has no decoder for — and reach nothing else: the play call has already
     * returned successfully by the time they happen.
     */
    val problem: StateFlow<String?> = mutableProblem.asStateFlow()

    /** Dismiss the current [problem]. */
    fun clearProblem() { mutableProblem.value = null }
    val hasActivePlayback: Boolean get() = mutableState.value.duration > 0.0 && !mutableState.value.ended

    override fun attachSurface(surface: Surface) {
        withOpenHandle { nativeAttachSurface(it, surface) }
    }

    override fun detachSurface() {
        withOpenHandle { nativeDetachSurface(it) }
    }

    override suspend fun play(
        media: String,
        nativeHandle: ULong,
        size: ULong,
        startPosition: Double,
        audioLanguage: String?,
        subtitleLanguage: String?,
        subtitles: Boolean,
        hardwareDecoding: Boolean,
    ) {
        check(size > 0uL) { "Cannot play an empty stream" }
        try {
            withContext(Dispatchers.IO) {
                val loaded = withOpenHandle {
                    nativeLoad(
                        it, nativeHandle.toLong(), startPosition, audioLanguage, subtitleLanguage, subtitles,
                        hardwareDecoding,
                    ).also { loaded ->
                        if (loaded) {
                            // The outgoing stream is released by mpv closing it,
                            // not from here: `loadfile` is asynchronous and its
                            // demuxer may still be reading. `stream_close` in
                            // `pstr_mpv.cpp` owns that release.
                            //
                            // The outgoing file's readings describe an episode
                            // that is no longer open. Retire them here as well
                            // as natively, so nothing observes them even once.
                            currentMedia = media
                            currentGeneration = withOpenHandle(::nativeState)?.generation() ?: -1.0
                            mutableState.value = MpvPlaybackState(
                                position = startPosition,
                                volume = mutableState.value.volume,
                                muted = mutableState.value.muted,
                                media = media,
                            )
                            mutableProblem.value = null
                            mutableTracks.value = emptyList()
                            mutableChapters.value = ChapterPlan(emptyList(), null)
                            plannedFor = null
                        }
                    }
                } ?: false
                check(loaded) {
                    "libmpv rejected the stream or no video surface was available"
                }
            }
            withOpenHandle {
                onPlaybackStarted?.invoke()
                startPolling()
            }
        } finally {
            // Whether the next episode arrived or failed to, the gap the
            // foreground service was holding open is over.
            mutableAdvancing.value = false
        }
    }

    /**
     * Declare that the player is walking to another episode.
     *
     * Called before the stream for the next one is opened, and cleared when
     * [play] returns either way — the window this covers is the one in which
     * nothing is loaded and the service would otherwise conclude playback is
     * finished.
     */
    fun beginTransition() { mutableAdvancing.value = true }

    /** Names the open episode for the media session, its notification and PiP. */
    fun publish(playing: NowPlaying?) { mutableNowPlaying.value = playing }

    fun setPaused(paused: Boolean) {
        // The native side writes the flag through, so every transport can flip
        // now instead of on the next poll a quarter second from here.
        val next = withOpenHandle {
            nativePause(it, paused)
            mutableState.value.copy(paused = paused).also { state -> mutableState.value = state }
        } ?: return
        onStateChanged?.invoke(next)
    }

    fun seek(position: Double) { withOpenHandle { nativeSeek(it, position) } }
    fun setVolume(volume: Double) { withOpenHandle { nativeVolume(it, volume) } }
    fun setMuted(muted: Boolean) { withOpenHandle { nativeMute(it, muted) } }

    /** Playback rate, 1.0 being the file's own. Clamped native-side. */
    fun setSpeed(speed: Double) { withOpenHandle { nativeSpeed(it, speed) } }
    fun selectTrack(track: MpvTrack?) {
        val audio = track?.type == "audio"
        withOpenHandle { nativeSelectTrack(it, audio, track?.id ?: -1) }
        refreshTracks()
    }

    /**
     * Explicit user stop: stop media and retire the foreground service.
     *
     * The stream behind the open file is released by mpv closing it, which
     * `stop` causes; releasing it from here would race that close.
     */
    fun stop() {
        val callback = withOpenHandle {
            nativeStop(it)
            // Nothing loaded means nothing to attribute a reading to: a save
            // issued after this must not land on the episode just stopped.
            currentMedia = null
            mutableAdvancing.value = false
            mutableChapters.value = ChapterPlan(emptyList(), null)
            mutableNowPlaying.value = null
            onExplicitStop
        }
        callback?.invoke()
    }

    override fun releaseNativeStream(nativeHandle: ULong) = pstrAndroidStreamRelease(nativeHandle.toLong())

    private fun startPolling() {
        if (poller?.isActive == true) return
        // Off the main thread deliberately: `nativeState` and `nativeTracks`
        // are synchronous `mpv_get_property` calls — five per track for the
        // latter — that contend on the core lock the demuxer holds during a
        // fetch. A poll that blocks there blocks the UI thread with it.
        poller = scope.launch(Dispatchers.Default) {
            var tick = 0
            while (isActive) {
                val values = withOpenHandle(::nativeState) ?: break
                if (values.size >= FIELDS) {
                    val next = MpvPlaybackState(
                        values[0], values[1], values[2],
                        values[3] != 0.0, values[4] != 0.0, values[5] != 0.0,
                        // Only a reading of the file this host was last asked
                        // to load carries its episode's name.
                        media = currentMedia.takeIf { values.generation() == currentGeneration },
                        videoWidth = values[WIDTH],
                        videoHeight = values[HEIGHT],
                        endReason = EndReason.of(values[END_REASON]),
                        buffering = values[BUFFERING] != 0.0,
                        cachePercent = values[CACHE_PERCENT],
                        seeking = values[SEEKING] != 0.0,
                        cachedUntil = values[CACHED_UNTIL],
                    )
                    mutableState.value = next
                    // The listeners drive the media session and the player UI,
                    // both of which are the main thread's.
                    withContext(Dispatchers.Main) { onStateChanged?.invoke(next) }
                    refreshChapters(next)
                }
                withOpenHandle(::nativeTakeError)?.let { mutableProblem.value = it }
                if (tick++ % 4 == 0) refreshTracks()
                delay(250)
            }
        }
    }

    /**
     * Re-read the chapters when there is a reason to: a different file, or a
     * duration that has only just arrived. Both change the answer — the length
     * of a chapter is what settles whether `Intro` is an opening or ten minutes
     * of story — and neither changes again after that, so this settles quickly
     * and then costs one comparison per poll.
     */
    private fun refreshChapters(state: MpvPlaybackState) {
        val generation = currentGeneration
        val wanted = state.duration to generation.toRawBits()
        if (plannedFor == wanted) return
        runCatching {
            val encoded = withOpenHandle(::nativeChapters) ?: return
            val values = JSONArray(encoded)
            List(values.length()) { index ->
                values.getJSONObject(index).run {
                    ChapterRecord(
                        index = getLong("index"),
                        title = optString("title").takeIf(String::isNotBlank),
                        start = getDouble("start"),
                    )
                }
            }
        }.onSuccess { chapters ->
            plannedFor = wanted
            mutableChapters.value = chapterPlan(chapters, state.duration.takeIf { it > 0.0 })
        }
    }

    private fun refreshTracks() {
        runCatching {
            val encoded = withOpenHandle(::nativeTracks) ?: return
            val values = JSONArray(encoded)
            List(values.length()) { index ->
                values.getJSONObject(index).run {
                    MpvTrack(getLong("id"), getString("type"), getString("language"), getString("title"), getBoolean("selected"))
                }
            }
        }.onSuccess { mutableTracks.value = it }
    }

    override fun close() {
        scope.cancel()
        val destroy = lifecycle.withLock {
            if (closed) return@withLock false
            closed = true
            onPlaybackStarted = null
            onStateChanged = null
            onExplicitStop = null
            onSkipPrevious = null
            onSkipNext = null
            mutableNowPlaying.value = null
            mutableAdvancing.value = false
            true
        }
        if (!destroy) return
        // `mpv_terminate_destroy` joins mpv's demuxer thread, which may be
        // parked in a block fetch over a degraded connection. This is called
        // from Service.onDestroy — on the main thread — so waiting for it there
        // is an ANR. `closed` is already set, so nothing new enters the handle;
        // the lock is only to let a call that is already inside finish.
        //
        // Terminating the core closes the open file, and closing it releases the
        // stream — after mpv's last read rather than during it.
        Thread({ lifecycle.withLock { nativeDestroy(nativeHandle) } }, "pstr-mpv-teardown").start()
    }

    /** Serializes destruction against every JNI use of the raw Player pointer. */
    private inline fun <T> withOpenHandle(block: (Long) -> T): T? = lifecycle.withLock {
        if (closed) null else block(nativeHandle)
    }

    private external fun nativeDestroy(handle: Long)
    private external fun nativeAttachSurface(handle: Long, surface: Surface)
    private external fun nativeDetachSurface(handle: Long)
    private external fun nativeLoad(
        handle: Long,
        stream: Long,
        start: Double,
        audio: String?,
        subtitle: String?,
        subtitles: Boolean,
        hardwareDecoding: Boolean,
    ): Boolean
    private external fun nativePause(handle: Long, paused: Boolean)
    private external fun nativeSeek(handle: Long, position: Double)
    private external fun nativeVolume(handle: Long, volume: Double)
    private external fun nativeSpeed(handle: Long, speed: Double)
    private external fun nativeMute(handle: Long, muted: Boolean)
    private external fun nativeSelectTrack(handle: Long, audio: Boolean, track: Long)
    private external fun nativeStop(handle: Long)
    private external fun nativeState(handle: Long): DoubleArray
    private external fun nativeTakeError(handle: Long): String?
    // Nullable: `NewStringUTF` returns null when the string cannot be built,
    // and a non-null declaration turns that into an NPE that `runCatching`
    // swallows into a silently empty track list.
    private external fun nativeTracks(handle: Long): String?
    private external fun nativeChapters(handle: Long): String?
    private external fun pstrAndroidStreamRelease(handle: Long)

    private fun DoubleArray.generation(): Double = getOrElse(GENERATION) { -1.0 }

    companion object {
        /** Fields `nativeState` fills, in the order `pstr_mpv.cpp` writes them. */
        private const val FIELDS = 14
        private const val GENERATION = 6
        private const val WIDTH = 7
        private const val HEIGHT = 8
        private const val END_REASON = 9
        private const val BUFFERING = 10
        private const val CACHE_PERCENT = 11
        private const val SEEKING = 12
        private const val CACHED_UNTIL = 13

        init {
            System.loadLibrary("pstr_android")
            System.loadLibrary("pstr_mpv")
        }

        /**
         * A host over a live libmpv core, or null if libmpv would not start.
         *
         * Null rather than an exception: the service that owns this is created
         * by the system, so a throw out of the constructor takes the process
         * with it — and the app has a graceful "this build does not include
         * libmpv" screen that a crash makes unreachable.
         */
        fun createOrNull(): NativeMpvHost? = nativeCreate().takeIf { it != 0L }?.let(::NativeMpvHost)

        @JvmStatic private external fun nativeCreate(): Long
    }
}
