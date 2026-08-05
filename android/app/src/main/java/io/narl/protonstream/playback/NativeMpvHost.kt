package io.narl.protonstream.playback

import android.view.Surface
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

data class MpvPlaybackState(
    val position: Double = 0.0,
    val duration: Double = 0.0,
    val volume: Double = 100.0,
    val paused: Boolean = true,
    val muted: Boolean = false,
    val ended: Boolean = false,
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
class NativeMpvHost : LibmpvHost, AutoCloseable {
    private val nativeHandle = nativeCreate().also { check(it != 0L) { "libmpv initialization failed" } }
    private val lifecycle = ReentrantLock()
    @Volatile private var closed = false
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val mutableState = MutableStateFlow(MpvPlaybackState())
    private val mutableTracks = MutableStateFlow(emptyList<MpvTrack>())
    private var poller: Job? = null
    private var currentStreamHandle: Long? = null
    var onPlaybackStarted: (() -> Unit)? = null
    var onStateChanged: ((MpvPlaybackState) -> Unit)? = null
    var onExplicitStop: (() -> Unit)? = null

    val state: StateFlow<MpvPlaybackState> = mutableState.asStateFlow()
    val tracks: StateFlow<List<MpvTrack>> = mutableTracks.asStateFlow()
    val hasActivePlayback: Boolean get() = mutableState.value.duration > 0.0 && !mutableState.value.ended

    override fun attachSurface(surface: Surface) {
        withOpenHandle { nativeAttachSurface(it, surface) }
    }

    override fun detachSurface() {
        withOpenHandle { nativeDetachSurface(it) }
    }

    override suspend fun play(
        nativeHandle: ULong,
        size: ULong,
        startPosition: Double,
        audioLanguage: String?,
        subtitleLanguage: String?,
        subtitles: Boolean,
    ) {
        check(size > 0uL) { "Cannot play an empty stream" }
        withContext(Dispatchers.IO) {
            val loaded = withOpenHandle {
                nativeLoad(it, nativeHandle.toLong(), startPosition, audioLanguage, subtitleLanguage, subtitles).also { loaded ->
                    if (loaded) {
                        currentStreamHandle?.let(::pstrAndroidStreamRelease)
                        currentStreamHandle = nativeHandle.toLong()
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
    }

    fun setPaused(paused: Boolean) { withOpenHandle { nativePause(it, paused) } }
    fun seek(position: Double) { withOpenHandle { nativeSeek(it, position) } }
    fun setVolume(volume: Double) { withOpenHandle { nativeVolume(it, volume) } }
    fun setMuted(muted: Boolean) { withOpenHandle { nativeMute(it, muted) } }
    fun selectTrack(track: MpvTrack?) {
        val audio = track?.type == "audio"
        withOpenHandle { nativeSelectTrack(it, audio, track?.id ?: -1) }
        refreshTracks()
    }

    /** Explicit user stop: stop media, release its stream, and retire the foreground service. */
    fun stop() {
        val callback = withOpenHandle {
            nativeStop(it)
            currentStreamHandle?.let(::pstrAndroidStreamRelease)
            currentStreamHandle = null
            onExplicitStop
        }
        callback?.invoke()
    }

    internal fun releaseEndedStream() {
        withOpenHandle { currentStreamHandle?.let(::pstrAndroidStreamRelease); currentStreamHandle = null }
    }

    override fun releaseNativeStream(nativeHandle: ULong) = pstrAndroidStreamRelease(nativeHandle.toLong())

    private fun startPolling() {
        if (poller?.isActive == true) return
        poller = scope.launch {
            var tick = 0
            while (isActive) {
                val values = withOpenHandle(::nativeState) ?: break
                if (values.size >= 6) {
                    val next = MpvPlaybackState(values[0], values[1], values[2], values[3] != 0.0, values[4] != 0.0, values[5] != 0.0)
                    mutableState.value = next
                    onStateChanged?.invoke(next)
                }
                if (tick++ % 4 == 0) refreshTracks()
                delay(250)
            }
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
        lifecycle.withLock {
            if (closed) return@withLock
            closed = true
            onPlaybackStarted = null
            onStateChanged = null
            onExplicitStop = null
            nativeDestroy(nativeHandle)
            currentStreamHandle?.let(::pstrAndroidStreamRelease)
            currentStreamHandle = null
        }
    }

    /** Serializes destruction against every JNI use of the raw Player pointer. */
    private inline fun <T> withOpenHandle(block: (Long) -> T): T? = lifecycle.withLock {
        if (closed) null else block(nativeHandle)
    }

    private external fun nativeCreate(): Long
    private external fun nativeDestroy(handle: Long)
    private external fun nativeAttachSurface(handle: Long, surface: Surface)
    private external fun nativeDetachSurface(handle: Long)
    private external fun nativeLoad(handle: Long, stream: Long, start: Double, audio: String?, subtitle: String?, subtitles: Boolean): Boolean
    private external fun nativePause(handle: Long, paused: Boolean)
    private external fun nativeSeek(handle: Long, position: Double)
    private external fun nativeVolume(handle: Long, volume: Double)
    private external fun nativeMute(handle: Long, muted: Boolean)
    private external fun nativeSelectTrack(handle: Long, audio: Boolean, track: Long)
    private external fun nativeStop(handle: Long)
    private external fun nativeState(handle: Long): DoubleArray
    private external fun nativeTracks(handle: Long): String
    private external fun pstrAndroidStreamRelease(handle: Long)

    companion object {
        init {
            System.loadLibrary("pstr_android")
            System.loadLibrary("pstr_mpv")
        }
    }
}
