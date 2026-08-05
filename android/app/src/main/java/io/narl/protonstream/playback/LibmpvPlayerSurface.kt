package io.narl.protonstream.playback

import android.view.Surface
import android.view.SurfaceHolder
import android.view.SurfaceView
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Slider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import io.narl.protonstream.native.NativeRuntime
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.pstr_android.AndroidEngine
import uniffi.pstr_android.AndroidStream
import uniffi.pstr_android.EpisodeRecord
import kotlin.math.roundToInt

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

    suspend fun closeEngine() = withContext(Dispatchers.IO) {
        engine?.releaseStream(episode.shareId, episode.volumeId, episode.linkId)
        engine = null
    }
}

/** Implement only in the variant that bundles a pinned libmpv native artifact. */
interface LibmpvHost {
    fun attachSurface(surface: Surface)
    fun detachSurface()
    /** Consumes the opaque handle through pstr_android_stream_{read,size}. */
    suspend fun play(
        nativeHandle: ULong,
        size: ULong,
        startPosition: Double = 0.0,
        audioLanguage: String? = null,
        subtitleLanguage: String? = null,
        subtitles: Boolean = true,
    )
    /** Calls pstr_android_stream_release exactly once for a published handle. */
    fun releaseNativeStream(nativeHandle: ULong)
}

private data class PlaybackStartup(
    val nativeHandle: ULong,
    val size: ULong,
    val startPosition: Double,
    val audioLanguage: String?,
    val subtitleLanguage: String?,
    val subtitles: Boolean,
)

@Composable
fun LibmpvPlayerSurface(titleKey: String, episode: EpisodeRecord, host: LibmpvHost? = null) {
    if (host == null) {
        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            Text("Playback is unavailable: this build does not include libmpv.", color = Color.White)
        }
        return
    }

    val scope = rememberCoroutineScope()
    var playbackError by remember(sessionKey(episode)) { mutableStateOf<String?>(null) }
    val session = remember(episode.shareId, episode.linkId) {
        RustStreamSession(episode)
    }
    LaunchedEffect(session, host) {
        runCatching {
            val startup = withContext(Dispatchers.IO) {
                session.open()
                val engine = NativeRuntime.engine()
                val preferences = engine.titleTrackPreferences(titleKey)
                val watch = engine.watchState(episode.shareId, episode.linkId)
                PlaybackStartup(
                    nativeHandle = session.nativeHandle(),
                    size = session.size(),
                    startPosition = watch?.positionSecs ?: 0.0,
                    audioLanguage = preferences.audioLanguage,
                    subtitleLanguage = preferences.subtitleLanguage,
                    subtitles = preferences.subtitles,
                )
            }
            host.play(
                startup.nativeHandle, startup.size, startup.startPosition,
                startup.audioLanguage, startup.subtitleLanguage, startup.subtitles,
            )
            session.transferToPlayer()
        }.onFailure { playbackError = it.message ?: "Playback failed" }
    }
    DisposableEffect(session, host) {
        onDispose {
            host.detachSurface()
            session.releaseNative(host)
            scope.launch { session.closeEngine() }
        }
    }
    Box(Modifier.fillMaxSize()) {
        AndroidView(
            modifier = Modifier.fillMaxSize(),
            factory = { context ->
                SurfaceView(context).apply {
                    holder.addCallback(object : SurfaceHolder.Callback {
                        override fun surfaceCreated(holder: SurfaceHolder) = host.attachSurface(holder.surface)
                        override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) = host.attachSurface(holder.surface)
                        override fun surfaceDestroyed(holder: SurfaceHolder) = host.detachSurface()
                    })
                }
            },
        )
        if (host is NativeMpvHost) PlayerControls(titleKey, episode, host, Modifier.align(Alignment.BottomCenter))
        playbackError?.let { message ->
            Text(message, color = Color.White, modifier = Modifier.align(Alignment.Center).background(Color.Black.copy(alpha = 0.8f)).padding(16.dp))
        }
    }
}

private fun sessionKey(episode: EpisodeRecord) = "${episode.shareId}:${episode.linkId}"

@Composable
private fun PlayerControls(titleKey: String, episode: EpisodeRecord, host: NativeMpvHost, modifier: Modifier = Modifier) {
    val state by host.state.collectAsState()
    val tracks by host.tracks.collectAsState()
    val scope = rememberCoroutineScope()
    var choosing by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(state.position.roundToInt() / 5) {
        if (state.duration > 0.0) withContext(Dispatchers.IO) {
            NativeRuntime.engine().saveWatchState(
                episode.shareId, episode.linkId, state.position.coerceAtMost(state.duration), state.duration,
                state.ended || state.position >= state.duration * 0.9,
            )
        }
    }
    Column(modifier.fillMaxWidth().background(Color.Black.copy(alpha = 0.68f)).padding(12.dp)) {
        Slider(
            value = state.position.toFloat().coerceIn(0f, state.duration.toFloat().coerceAtLeast(1f)),
            onValueChange = { host.seek(it.toDouble()) },
            valueRange = 0f..state.duration.toFloat().coerceAtLeast(1f),
        )
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
            Button(onClick = { host.setPaused(!state.paused) }) { Text(if (state.paused) "Play" else "Pause") }
            Text("${formatTime(state.position)} / ${formatTime(state.duration)}", color = Color.White)
            Button(onClick = { host.setMuted(!state.muted) }) { Text(if (state.muted) "Unmute" else "Mute") }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Button(onClick = { choosing = "audio" }) {
                Text("Audio: ${tracks.firstOrNull { it.type == "audio" && it.selected }?.label ?: "default"}", maxLines = 1)
            }
            Button(onClick = { choosing = "sub" }) {
                Text("Subtitles: ${tracks.firstOrNull { it.type == "sub" && it.selected }?.label ?: "off"}", maxLines = 1)
            }
        }
    }
    choosing?.let { type ->
        TrackChooser(
            type = type,
            tracks = tracks.filter { it.type == type },
            onDismiss = { choosing = null },
            onSelect = { selected ->
                host.selectTrack(selected)
                choosing = null
                scope.launch(Dispatchers.IO) {
                    val engine = NativeRuntime.engine()
                    val old = engine.titleTrackPreferences(titleKey)
                    engine.setTitleTrackPreferences(
                        titleKey,
                        if (type == "audio") old.copy(audioLanguage = selected?.language?.takeIf(String::isNotBlank))
                        else old.copy(
                            subtitleLanguage = selected?.language?.takeIf(String::isNotBlank),
                            subtitles = selected != null,
                        ),
                    )
                }
            },
        )
    }
}

@Composable
private fun TrackChooser(
    type: String,
    tracks: List<MpvTrack>,
    onDismiss: () -> Unit,
    onSelect: (MpvTrack?) -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (type == "audio") "Audio track" else "Subtitle track") },
        text = {
            LazyColumn(Modifier.fillMaxWidth().heightIn(max = 420.dp)) {
                if (type == "sub") {
                    item {
                        OutlinedButton(onClick = { onSelect(null) }, Modifier.fillMaxWidth()) {
                            Text(if (tracks.none(MpvTrack::selected)) "✓  Off" else "Off")
                        }
                    }
                }
                items(tracks, key = MpvTrack::id) { track ->
                    OutlinedButton(onClick = { onSelect(track) }, Modifier.fillMaxWidth()) {
                        Text(if (track.selected) "✓  ${track.label}" else track.label)
                    }
                }
            }
        },
        confirmButton = { Button(onClick = onDismiss) { Text("Close") } },
    )
}

private fun formatTime(seconds: Double): String {
    val total = seconds.coerceAtLeast(0.0).roundToInt()
    return "%d:%02d".format(total / 60, total % 60)
}
