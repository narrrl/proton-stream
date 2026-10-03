package io.narl.protonstream.playback

import android.view.Surface
import io.narl.protonstream.native.NativeRuntime
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.pstr_android.AndroidEngine
import uniffi.pstr_android.AndroidStream
import uniffi.pstr_android.EpisodeRecord

/** Exact seekable stream contract a bundled libmpv JNI adapter must consume. */
class RustStreamSession(
    private val episode: EpisodeRecord,
) {
    private var engine: AndroidEngine? = null
    private var stream: AndroidStream? = null
    private var handle: ULong? = null
    private var streamSize: ULong? = null

    suspend fun open(): RustStreamSession = withContext(Dispatchers.IO) {
        val openedEngine = NativeRuntime.engine()
        engine = openedEngine
        stream = openedEngine.openStream(episode.shareId, episode.volumeId, episode.linkId).also {
            streamSize = it.size()
            handle = it.nativeHandle()
        }
        this@RustStreamSession
    }

    fun nativeHandle(): ULong = checkNotNull(handle) { "stream is not published" }

    fun size(): ULong = checkNotNull(streamSize) { "stream is not open" }

    fun releaseNative(host: LibmpvHost) {
        handle?.let(host::releaseNativeStream)
        handle = null
        streamSize = null
        stream = null
    }

    /** The native player now owns the published token until stop/end/close. */
    fun transferToPlayer() {
        handle = null
        streamSize = null
        stream = null
    }

    /**
     * Give the episode's stream back to Rust, from a scope of this session's own.
     *
     * Deliberately not the caller's: the one moment this has to run is when the
     * player leaves the composition, which is the same moment a
     * `rememberCoroutineScope` is cancelled — so a release launched there is
     * cancelled before it starts, and the stream is held for the life of the
     * process. Once per episode played, that is every episode.
     */
    fun close(host: LibmpvHost) {
        releaseNative(host)
        sessions.launch {
            runCatching { engine?.releaseStream(episode.shareId, episode.volumeId, episode.linkId) }
            engine = null
        }
    }
}

/**
 * Where stream teardown runs: process-lifetime, like the engine it releases to,
 * and never cancelled by a screen going away.
 */
private val sessions = CoroutineScope(SupervisorJob() + Dispatchers.IO)

/** Implement only in the variant that bundles a pinned libmpv native artifact. */
interface LibmpvHost {
    fun attachSurface(surface: Surface)
    fun detachSurface()

    /**
     * Consumes the opaque handle through pstr_android_stream_{read,size}.
     *
     * [media] names the episode being loaded. One host serves every episode in
     * turn, so its readings have to say which one they describe — see
     * [MpvPlaybackState.media].
     */
    suspend fun play(
        media: String,
        nativeHandle: ULong,
        size: ULong,
        startPosition: Double = 0.0,
        audioLanguage: String? = null,
        subtitleLanguage: String? = null,
        subtitles: Boolean = true,
        hardwareDecoding: Boolean = true,
    )

    /** Calls pstr_android_stream_release exactly once for a published handle. */
    fun releaseNativeStream(nativeHandle: ULong)
}

/** How an episode is named in watch state, work names and player readings. */
fun mediaKey(episode: EpisodeRecord) = "${episode.shareId}:${episode.linkId}"
