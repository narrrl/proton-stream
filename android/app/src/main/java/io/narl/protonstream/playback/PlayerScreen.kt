package io.narl.protonstream.playback

import android.app.Activity
import android.content.pm.ActivityInfo
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.WindowManager
import android.view.accessibility.AccessibilityManager
import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.Toc
import androidx.compose.material.icons.automirrored.filled.VolumeOff
import androidx.compose.material.icons.automirrored.filled.VolumeUp
import androidx.compose.material.icons.filled.Audiotrack
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Speed
import androidx.compose.material.icons.filled.Forward30
import androidx.compose.material.icons.filled.Fullscreen
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.FullscreenExit
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Replay10
import androidx.compose.material.icons.filled.SkipNext
import androidx.compose.material.icons.filled.SkipPrevious
import androidx.compose.material.icons.filled.Subtitles
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.semantics
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalWindowInfo
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import io.narl.protonstream.settings.SettingsStore
import io.narl.protonstream.native.NativeRuntime
import io.narl.protonstream.ui.theme.AccentButton
import io.narl.protonstream.ui.theme.AccentTrack
import io.narl.protonstream.ui.theme.EdgedButton
import io.narl.protonstream.ui.theme.QuietButton
import io.narl.protonstream.ui.theme.TonalButton
import io.narl.protonstream.ui.thumbnailSource
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material.icons.filled.VideoLibrary
import androidx.compose.foundation.layout.PaddingValues
import io.narl.protonstream.ui.theme.AccentProgress
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import io.narl.protonstream.ui.RemoteArtwork
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.ui.draw.clip
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.pstr_android.ChapterEntry
import uniffi.pstr_android.EpisodeRecord
import uniffi.pstr_android.TitleRecord
import uniffi.pstr_android.TrackPreferencesRecord
import uniffi.pstr_android.skipOffer
import kotlin.math.ceil
import kotlin.math.roundToInt

/** How long the controls stay up after a tap. */
private const val CONTROLS_SECONDS = 4L

/** How long the "up next" card counts down before it loads the next episode. */
private const val UP_NEXT_SECONDS = 10

/** How far a double tap on either side of the picture seeks. */
private const val DOUBLE_TAP_SECONDS = 10

/** How long the double-tap seek mark stays up. */
private const val NUDGE_MS = 700L

/** How close to the end the next episode's opening blocks are fetched. */
private const val WARM_NEXT_SECONDS = 120.0

/** Past this much of an episode it counts as watched. */
internal const val WATCHED_FRACTION = 0.9

/** The loudest this app plays. Matches `pstr_core::prefs`, which stores it. */
private const val MAX_VOLUME = 100f

/**
 * The narrowest layout the in-app volume slider is worth the room it costs.
 *
 * Below it — a phone held upright — the transport row is already five controls
 * wide, and the device's own volume keys are the better answer.
 */
private val VOLUME_SLIDER_MIN_WIDTH = 720.dp

/** Narrower than this, the pickers past subtitles go behind "More". */
private val INLINE_CONTROLS_MIN_WIDTH = 560.dp

/**
 * The player, as its own screen on the activity's own window.
 *
 * Not a dialog: a dialog window has its own insets and cannot be told to draw
 * into the display cutout. Owning the activity window is what makes
 * "fullscreen" mean the whole panel, notch included.
 *
 * [episodes] is the title's episodes in display order and [index] the one that is
 * open, which is what previous/next and autoplay walk. The index is hoisted so
 * that it survives process recreation with the rest of the navigation state —
 * this screen is destroyed and rebuilt by a dark-mode toggle like any other.
 */
@Composable
fun PlayerScreen(
    title: TitleRecord,
    episodes: List<EpisodeRecord>,
    index: Int,
    host: LibmpvHost?,
    minimized: Boolean,
    onIndexChange: (Int) -> Unit,
    onSaveProgress: (EpisodeRecord, Double, Double, Boolean) -> Unit,
    onMinimize: () -> Unit,
    onClose: () -> Unit,
) {
    val context = LocalContext.current
    val activity = context as? Activity
    val titleKey = title.key
    val episode = episodes.getOrNull(index)

    // Minimised, this screen keeps the stream and the effects that own it, and
    // draws nothing: no surface — mpv plays on without one, exactly as it does
    // while the app is in the background — no immersive window, and no back
    // handler, because back belongs to the page the viewer went to.
    if (!minimized) {
        ImmersiveWindow(activity)
        // Back leaves the video rather than ending it. Stopping is the mini
        // transport's ✕, which is the one place it is unambiguous.
        BackHandler(onBack = onMinimize)
    }

    // The transport reads state only this host publishes, so the interface
    // alone is not enough to draw a player.
    val nativeHost = host as? NativeMpvHost
    if (nativeHost == null || episode == null) {
        if (minimized) return
        Box(Modifier.fillMaxSize().background(Color.Black), contentAlignment = Alignment.Center) {
            Text(
                if (nativeHost == null) "Playback is unavailable: this build does not include libmpv."
                else "This title has no episode to play.",
                color = Color.White,
            )
        }
        return
    }

    val key = mediaKey(episode)
    // Retry opens the episode again from scratch: a new stream, through the
    // bridge's session refresh, from where the failed one stopped.
    var attempt by remember(key) { mutableIntStateOf(0) }
    var retryFrom by remember(key) { mutableStateOf<Double?>(null) }
    val session = remember(key, attempt) { RustStreamSession(episode) }
    var playbackError by remember(key) { mutableStateOf<String?>(null) }
    // From the click until mpv has the file: the share session, the revision
    // and its block list all come first, and on a slow link that is seconds of
    // black screen unless something says so.
    var opening by remember(key, attempt) { mutableStateOf(true) }

    LaunchedEffect(session, host) {
        runCatching {
            val startup = withContext(Dispatchers.IO) {
                session.open()
                val engine = NativeRuntime.engine()
                // A choice made on this title wins; without one, the choice the
                // viewer made anywhere else does. Both are the desktop client's
                // fallback chain, and it is what makes "Japanese with English
                // subtitles" a thing you say once rather than once per show.
                val show = engine.titleTrackPreferences(titleKey)
                val global = engine.playbackPrefs()
                val watch = engine.watchState(episode.shareId, episode.linkId)
                Startup(
                    nativeHandle = session.nativeHandle(),
                    size = session.size(),
                    // A finished episode restarts rather than resuming three
                    // seconds from its own credits.
                    startPosition = retryFrom ?: watch?.takeUnless { it.watched }?.positionSecs ?: 0.0,
                    audioLanguage = show?.audioLanguage ?: global.audioLanguage,
                    subtitleLanguage = show?.subtitleLanguage ?: global.subtitleLanguage,
                    subtitles = show?.subtitles ?: global.subtitles,
                    volume = global.volume,
                    muted = global.muted,
                    speed = global.speed,
                )
            }
            nativeHost.play(
                key, startup.nativeHandle, startup.size, startup.startPosition,
                startup.audioLanguage, startup.subtitleLanguage, startup.subtitles,
                SettingsStore(context).hardwareDecoding,
            )

            // After the load, because mpv keeps none of the three across one.
            nativeHost.setVolume(startup.volume)
            nativeHost.setMuted(startup.muted)
            nativeHost.setSpeed(startup.speed)
            session.transferToPlayer()
            // While mpv opens the episode, and off the spinner's path: the host
            // holds them until it has the file.
            launch(Dispatchers.IO) {
                val files = NativeRuntime.engine()
                    .openSubtitles(episode.shareId, episode.linkId, startup.subtitleLanguage)
                nativeHost.addSubtitles(key, files, startup.subtitles)
            }
        }.onFailure { playbackError = it.message ?: "Playback failed" }
        opening = false
    }

    // Per episode: the stream Rust holds open. The surface is the screen's, not
    // this episode's, so detaching it belongs to the effect below and not here.
    DisposableEffect(session) { onDispose { session.close(nativeHost) } }
    DisposableEffect(nativeHost) { onDispose { nativeHost.detachSurface() } }

    val state by nativeHost.state.collectAsState()
    val chapters by nativeHost.chapters.collectAsState()
    val tracks by nativeHost.tracks.collectAsState()
    // Read once: a preference the viewer changes mid-episode applies to the
    // next one, and re-reading the store per frame would not.
    var autoSkip by remember { mutableStateOf(false) }
    var autoplay by remember { mutableStateOf(true) }
    LaunchedEffect(Unit) {
        runCatching { withContext(Dispatchers.IO) { NativeRuntime.engine().playbackPrefs() } }
            .onSuccess { autoSkip = it.autoSkip; autoplay = it.autoplayNext }
    }

    // Resume position, written only from readings that carry this episode's
    // name: one host serves every episode, and an unattributed reading is the
    // previous one's clock.
    fun save() {
        val current = nativeHost.state.value
        if (current.media == key && current.duration > 0.0) {
            onSaveProgress(
                episode,
                current.position.coerceIn(0.0, current.duration),
                current.duration,
                // Watched means played to the end. An episode that was stopped,
                // or that mpv could not play at all, also "ended" — recording
                // either as watched retires an episode nobody saw.
                current.endReason == EndReason.Eof ||
                    current.position >= current.duration * WATCHED_FRACTION,
            )
        }
    }
    // The five-second save is PlaybackService's, which outlives this screen;
    // these are the moments only the screen sees.
    DisposableEffect(key) { onDispose { save() } }

    val next = episodes.getOrNull(index + 1)
    // Both walks tell the host they are one, so the foreground service holds
    // through the gap where nothing is loaded — see NativeMpvHost.advancing.
    fun advance() {
        save()
        next?.let { nativeHost.beginTransition(); onIndexChange(index + 1) }
    }
    fun retreat() {
        if (index == 0) return
        save()
        nativeHost.beginTransition()
        onIndexChange(index - 1)
    }
    fun jumpTo(target: Int) {
        if (target == index || target !in episodes.indices) return
        save()
        nativeHost.beginTransition()
        onIndexChange(target)
    }

    // What the media notification and the lock screen show. Neither can reach
    // this composition, and both have to name the episode rather than the app.
    LaunchedEffect(key, index, episodes.size, title.key) {
        nativeHost.publish(
            NowPlaying(
                media = key,
                title = episode.label,
                show = title.displayName,
                detail = episode.detail,
                artworkUrl = title.posterUrl ?: title.backdropUrl,
                artworkFile = episode.thumbnailSource,
                hasPrevious = index > 0,
                hasNext = next != null,
            ),
        )
    }
    // Those same surfaces walk the playlist through here, because this is where
    // the open episode lives. Both callbacks run on the main thread.
    DisposableEffect(nativeHost, index, episodes.size) {
        nativeHost.onSkipNext = { advance() }
        nativeHost.onSkipPrevious = { retreat() }
        onDispose {
            nativeHost.onSkipNext = null
            nativeHost.onSkipPrevious = null
        }
    }

    // Autoplay fires on a clean end of file only, and only for this episode's
    // own reading — never on a failure, and never on the outgoing file's.
    // Cancelled from the end-of-file card: the viewer chose to stay.
    var stay by remember(key) { mutableStateOf(false) }
    LaunchedEffect(state.endReason, state.media) {
        if (state.endReason == EndReason.Eof && state.media == key && autoplay && !stay && next != null) advance()
    }
    // A file mpv could not play ends like any other. Saying so is the whole
    // difference between a broken episode and an app that silently skipped one.
    LaunchedEffect(state.endReason, state.media) {
        if (state.endReason == EndReason.Failed && state.media == key) {
            playbackError = "This episode could not be played."
        }
    }
    // A stall that does not clear is a dropped connection far more often than
    // a slow one, and "Buffering…" for ever offers no way out of it.
    LaunchedEffect(state.buffering, key, attempt) {
        if (state.buffering) {
            delay(STALL_LIMIT_MS)
            playbackError = "Still loading. The connection may have dropped."
        }
    }
    // What mpv itself complained about, which the play call never sees: by the
    // time a header turns out to be unreadable it has long since returned.
    val problem by nativeHost.problem.collectAsState()
    LaunchedEffect(problem) { problem?.let { playbackError = it } }

    // Whether mpv's readings are this episode's. Not while the next one opens:
    // the last one's credits play on until it is loaded, and read against the
    // new index they offered the episode after the next as up next.
    val playingThis = state.media == key
    val offer = remember(chapters, key, playingThis, (state.position * 4).roundToInt()) {
        if (!playingThis) return@remember null
        runCatching { skipOffer(chapters, state.position) }.getOrNull()
    }
    // Near the end, fetch the next episode's opening blocks, so autoplay or
    // "Play now" starts from the cache. The last two minutes or the credits,
    // whichever comes first: late enough that stopping after one episode has
    // not cost a download, early enough to matter.
    val nearEnd = playingThis && state.duration > 0.0 && (
        state.duration - state.position < WARM_NEXT_SECONDS ||
            chapters.creditsStart?.let { state.position >= it } == true
        )
    LaunchedEffect(key, nearEnd) {
        val upcoming = next ?: return@LaunchedEffect
        if (!nearEnd) return@LaunchedEffect
        runCatching {
            NativeRuntime.engine().warmEpisode(upcoming.shareId, upcoming.volumeId, upcoming.linkId)
        }
    }
    // Auto-skip is offered once per chapter: seeking back into an opening on
    // purpose must not be undone by the setting that skipped it.
    val skipped = remember(key) { mutableSetOf<Long>() }
    LaunchedEffect(offer?.target) {
        val target = offer?.target ?: return@LaunchedEffect
        val chapter = chapters.entries.lastOrNull { state.position + 0.001 >= it.start } ?: return@LaunchedEffect
        if (autoSkip && !state.paused && skipped.add(chapter.index)) nativeHost.seek(target)
    }

    var landscapeLocked by remember { mutableStateOf(false) }
    // Set by the rotation button: from then on the viewer decides.
    var rotationChosen by remember { mutableStateOf(false) }
    // A wide picture fills a phone only on its side, so it turns there by
    // itself, once per episode, until the viewer picks for themselves.
    LaunchedEffect(key, playingThis, state.videoWidth > state.videoHeight) {
        if (!playingThis || rotationChosen || landscapeLocked) return@LaunchedEffect
        if (state.videoHeight > 0.0 && state.videoWidth > state.videoHeight) {
            landscapeLocked = activity?.lockLandscape() ?: false
        }
    }
    var showControls by remember { mutableStateOf(true) }
    // TalkBack moves between controls by swiping, and a control that fades
    // four seconds after it is reached cannot be found again.
    val exploring = rememberTouchExploration()
    LaunchedEffect(exploring) { if (exploring) showControls = true }
    LaunchedEffect(showControls, state.paused, exploring) {
        if (showControls && !state.paused && !exploring) {
            delay(CONTROLS_SECONDS * 1_000)
            showControls = false
        }
    }
    // Which side was double-tapped, shown for a moment so the seek is seen
    // to have happened; the count makes a second tap on one side show again.
    var nudge by remember { mutableStateOf<Pair<Int, Int>?>(null) }
    LaunchedEffect(nudge) {
        if (nudge != null) {
            delay(NUDGE_MS)
            nudge = null
        }
    }
    val current by rememberUpdatedState(state)
    if (minimized) return

    Box(
        Modifier
            .fillMaxSize()
            .background(Color.Black)
            // Double-tap a side to seek ten seconds, as every phone video
            // player has taught; the middle plays or pauses. A single tap
            // waits out the double-tap timeout, which is the price of both.
            .pointerInput(host) {
                detectTapGestures(
                    onTap = { showControls = !showControls },
                    onDoubleTap = { offset ->
                        val third = size.width / 3f
                        val seconds = when {
                            offset.x < third -> -DOUBLE_TAP_SECONDS
                            offset.x > size.width - third -> DOUBLE_TAP_SECONDS
                            else -> 0
                        }
                        if (seconds == 0) {
                            host.setPaused(!current.paused)
                        } else {
                            host.seek((current.position + seconds).coerceAtLeast(0.0))
                            nudge = seconds to (nudge?.second ?: 0) + 1
                        }
                    },
                )
            }
            .semantics {
                onClick(label = if (showControls) "Hide controls" else "Show controls") {
                    showControls = !showControls
                    true
                }
            },
    ) {
        AndroidView(
            modifier = Modifier.fillMaxSize(),
            factory = { viewContext ->
                SurfaceView(viewContext).apply {
                    holder.addCallback(object : SurfaceHolder.Callback {
                        override fun surfaceCreated(holder: SurfaceHolder) = host.attachSurface(holder.surface)
                        override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) =
                            host.attachSurface(holder.surface)
                        override fun surfaceDestroyed(holder: SurfaceHolder) = host.detachSurface()
                    })
                }
            },
        )

        AnimatedVisibility(
            visible = showControls,
            modifier = Modifier.align(Alignment.TopStart),
            enter = fadeIn(),
            exit = fadeOut(),
        ) {
            Row(
                Modifier
                    .fillMaxWidth()
                    .background(MaterialTheme.colorScheme.scrim.copy(alpha = 0.45f))
                    .windowInsetsPadding(WindowInsets.safeDrawing)
                    .padding(horizontal = 8.dp, vertical = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                IconButton(onClick = onMinimize) {
                    Icon(
                        Icons.AutoMirrored.Filled.ArrowBack,
                        contentDescription = "Leave the video",
                        tint = Color.White,
                    )
                }
                // The episode leads and the show follows it: which episode is
                // the thing a viewer who has just woken the controls is
                // checking, and the show is what they already know.
                Column(Modifier.weight(1f)) {
                    Text(
                        episode.label,
                        color = Color.White,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        style = MaterialTheme.typography.titleMedium,
                    )
                    Text(
                        listOfNotNull(title.displayName, episode.detail.takeIf(String::isNotBlank))
                            .joinToString("  •  "),
                        color = Color.White.copy(alpha = 0.7f),
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                // Two ways out, because they mean different things: back leaves
                // the video and keeps the episode playing, this ends it.
                IconButton(onClick = onClose) {
                    Icon(Icons.Default.Close, contentDescription = "Stop playback", tint = Color.White)
                }
                remaining(state)?.let {
                    Text(
                        it,
                        color = Color.White.copy(alpha = 0.7f),
                        style = MaterialTheme.typography.labelMedium,
                        modifier = Modifier.padding(start = 12.dp, end = 4.dp),
                    )
                }
            }
        }

        AnimatedVisibility(
            visible = showControls,
            modifier = Modifier.align(Alignment.BottomCenter),
            enter = fadeIn(),
            exit = fadeOut(),
        ) {
            PlayerControls(
                titleKey = titleKey,
                host = nativeHost,
                state = state,
                chapters = chapters.entries,
                tracks = tracks,
                hasPrevious = index > 0,
                hasNext = next != null,
                onPrevious = { retreat() },
                onNext = { advance() },
                episodes = episodes,
                playing = index,
                onEpisode = { jumpTo(it) },
                onRotate = {
                    rotationChosen = true
                    landscapeLocked = activity?.toggleLandscape() ?: false
                },
                landscapeLocked = landscapeLocked,
            )
        }

        // Drawn whether or not the rest of the controls are up: an opening lasts
        // ninety seconds, and a button you have to wake with a tap is one nobody
        // reaches in time.
        offer?.takeIf { !autoSkip }?.let { skip ->
            TonalButton(
                onClick = { nativeHost.seek(skip.target) },
                modifier = Modifier
                    .align(Alignment.BottomEnd)
                    .windowInsetsPadding(WindowInsets.safeDrawing)
                    .padding(end = 16.dp, bottom = if (showControls) 132.dp else 24.dp),
            ) { Text(skip.label) }
        }

        nudge?.let { (seconds, _) ->
            Box(
                Modifier
                    .align(if (seconds < 0) Alignment.CenterStart else Alignment.CenterEnd)
                    .padding(horizontal = 48.dp)
                    .size(72.dp)
                    .clip(CircleShape)
                    .background(MaterialTheme.colorScheme.scrim.copy(alpha = 0.5f)),
                contentAlignment = Alignment.Center,
            ) {
                Text(if (seconds < 0) "−${-seconds} s" else "+$seconds s", color = Color.White)
            }
        }

        // A file with no chapters has no credits to count over, so the card
        // takes its last seconds instead: without it the next episode cut in
        // with no warning and no way to stay. Counted on the file's own clock,
        // so it reaches zero at the end of file, which is what advances.
        val tailLeft = (state.duration - state.position).takeIf {
            chapters.creditsStart == null && state.duration > UP_NEXT_SECONDS * 6 && it <= UP_NEXT_SECONDS
        }
        UpNextCard(
            next = next,
            visible = autoplay && !stay && !state.paused && !state.ended && playingThis &&
                (tailLeft != null || chapters.creditsStart?.let { state.position >= it } == true),
            secondsLeft = tailLeft?.let { ceil(it).toInt() },
            onPlayNow = { advance() },
            onStay = { stay = true },
            modifier = Modifier
                .align(Alignment.BottomEnd)
                .windowInsetsPadding(WindowInsets.safeDrawing)
                .padding(end = 16.dp, bottom = if (showControls) 132.dp else 24.dp),
        )

        // Played to the end with nothing starting after it: the last episode,
        // autoplay off, or the viewer stayed. A black screen said none of that.
        if (playingThis && state.endReason == EndReason.Eof && (next == null || !autoplay || stay)) {
            EndCard(
                finished = episode.label,
                show = title.displayName,
                next = next,
                onPlayNext = { advance() },
                // mpv unloads a file at its end, so this opens it again.
                onReplay = { retryFrom = 0.0; attempt++ },
                onClose = onClose,
                modifier = Modifier.align(Alignment.Center),
            )
        }

        // Only while nothing else explains the still picture: an error is the
        // more useful thing to say, and a spinner over it reads as "still
        // trying".
        if ((opening || state.stalled) && playbackError == null) {
            Column(
                Modifier
                    .align(Alignment.Center)
                    .background(MaterialTheme.colorScheme.surfaceContainerHighest, MaterialTheme.shapes.small)
                    .padding(20.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                CircularProgressIndicator(
                    color = MaterialTheme.colorScheme.primary,
                    modifier = Modifier.size(36.dp),
                )
                if (opening || state.buffering) {
                    Text(
                        when {
                            opening -> "Opening ${episode.label}…"
                            state.cachePercent > 0.0 -> "Buffering… ${state.cachePercent.roundToInt()}%"
                            else -> "Buffering…"
                        },
                        color = MaterialTheme.colorScheme.onSurface,
                        style = MaterialTheme.typography.labelMedium,
                        modifier = Modifier.padding(top = 12.dp),
                    )
                }
            }
        }

        playbackError?.let { message ->
            Column(
                Modifier
                    .align(Alignment.Center)
                    .background(MaterialTheme.colorScheme.errorContainer, MaterialTheme.shapes.small)
                    .padding(16.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Text(message, color = MaterialTheme.colorScheme.onErrorContainer)
                // Dismissible, because the message is not always fatal — a
                // subtitle track that failed to load leaves an episode that
                // plays perfectly well behind an error that used to be permanent.
                Row {
                    QuietButton(onClick = { playbackError = null; nativeHost.clearProblem() }) {
                        Text("Dismiss", color = MaterialTheme.colorScheme.onErrorContainer)
                    }
                    QuietButton(
                        onClick = {
                            retryFrom = state.position.takeIf { state.media == key && it > 0.0 } ?: retryFrom
                            playbackError = null
                            nativeHost.clearProblem()
                            attempt++
                        },
                    ) {
                        Text("Retry", color = MaterialTheme.colorScheme.onErrorContainer)
                    }
                }
            }
        }
    }
}

/** How long a stall lasts before the player offers to open the episode again. */
private const val STALL_LIMIT_MS = 20_000L

private data class Startup(
    val nativeHandle: ULong,
    val size: ULong,
    val startPosition: Double,
    val audioLanguage: String?,
    val subtitleLanguage: String?,
    val subtitles: Boolean,
    val volume: Double,
    val muted: Boolean,
    val speed: Double,
)

/**
 * Immersive for as long as this screen is up, and only that long.
 *
 * Hiding the bars is not enough on its own: without `setDecorFitsSystemWindows`
 * and a cutout mode the picture stops at the notch and at the gesture bar, which
 * is the "not really fullscreen" everyone notices.
 */
@Composable
private fun ImmersiveWindow(activity: Activity?) {
    DisposableEffect(activity) {
        val window = activity?.window
        if (window == null) return@DisposableEffect onDispose {}
        val controller = WindowInsetsControllerCompat(window, window.decorView)
        val previousCutout = window.attributes.layoutInDisplayCutoutMode
        WindowCompat.setDecorFitsSystemWindows(window, false)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        window.attributes = window.attributes.also {
            it.layoutInDisplayCutoutMode =
                WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
        }
        controller.hide(WindowInsetsCompat.Type.systemBars())
        controller.systemBarsBehavior =
            WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        onDispose {
            controller.show(WindowInsetsCompat.Type.systemBars())
            window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            window.attributes = window.attributes.also { it.layoutInDisplayCutoutMode = previousCutout }
            activity.requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED
        }
    }
}

/** Lock to landscape. Reports whether it is. */
private fun Activity.lockLandscape(): Boolean {
    requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
    return true
}

/** Whether TalkBack, or anything else that explores by touch, is on. */
@Composable
private fun rememberTouchExploration(): Boolean {
    val context = LocalContext.current
    val manager = remember(context) { context.getSystemService(AccessibilityManager::class.java) }
    var exploring by remember { mutableStateOf(manager?.isTouchExplorationEnabled == true) }
    DisposableEffect(manager) {
        val listener = AccessibilityManager.TouchExplorationStateChangeListener { exploring = it }
        manager?.addTouchExplorationStateChangeListener(listener)
        onDispose { manager?.removeTouchExplorationStateChangeListener(listener) }
    }
    return exploring
}

/** Lock to landscape, or hand rotation back to the sensor. Reports which. */
private fun Activity.toggleLandscape(): Boolean {
    val lock = requestedOrientation != ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
    requestedOrientation = if (lock) ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
    else ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED
    return lock
}

/**
 * Everything the transport offers, in two rows.
 *
 * A phone in landscape has one thumb's worth of room, so the second row is
 * icons: the transport cluster on the left, where a thumb already is, and the
 * pickers on the right. Only the seek bar gets a full row of its own.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun PlayerControls(
    titleKey: String,
    host: NativeMpvHost,
    state: MpvPlaybackState,
    chapters: List<ChapterEntry>,
    tracks: List<MpvTrack>,
    hasPrevious: Boolean,
    hasNext: Boolean,
    onPrevious: () -> Unit,
    onNext: () -> Unit,
    episodes: List<EpisodeRecord>,
    playing: Int,
    onEpisode: (Int) -> Unit,
    onRotate: () -> Unit,
    landscapeLocked: Boolean,
) {
    val scope = rememberCoroutineScope()
    var choosing by remember { mutableStateOf<String?>(null) }
    var showEpisodes by remember { mutableStateOf(false) }
    var showChapters by remember { mutableStateOf(false) }
    var showSpeeds by remember { mutableStateOf(false) }
    // What the store says, so the dialog opens on the rate that is playing.
    var speed by remember { mutableStateOf(1.0) }
    LaunchedEffect(Unit) {
        runCatching { withContext(Dispatchers.IO) { NativeRuntime.engine().playbackPrefs() } }
            .onSuccess { speed = it.speed }
    }
    val duration = state.duration.toFloat().coerceAtLeast(1f)
    val density = LocalDensity.current
    val width = with(density) { LocalWindowInfo.current.containerSize.width.toDp() }
    val roomForVolume = width >= VOLUME_SLIDER_MIN_WIDTH
    // A phone in portrait has room for the transport and two more buttons at
    // a thumb's size. The rest go behind "More" there rather than off the edge.
    val roomForAll = width >= INLINE_CONTROLS_MIN_WIDTH
    val toggleMute = {
        host.setMuted(!state.muted)
        scope.launch(Dispatchers.IO) { rememberVolume(muted = !state.muted) }
        Unit
    }

    Column(
        Modifier
            .fillMaxWidth()
            .background(MaterialTheme.colorScheme.scrim.copy(alpha = 0.68f))
            .windowInsetsPadding(WindowInsets.safeDrawing)
            .padding(horizontal = 12.dp, vertical = 4.dp),
    ) {
        // Where the thumb is while a finger is on it. A seek per touch-move
        // aborts every outstanding prefetch, so a one-second drag used to issue
        // dozens of seeks and throw away every in-flight block — and the thumb
        // snapped backwards under the finger, because it was drawn from a
        // position that only updates four times a second.
        var scrubbing by remember { mutableStateOf<Float?>(null) }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                formatTime(scrubbing?.toDouble() ?: state.position),
                color = Color.White,
                style = MaterialTheme.typography.labelMedium,
            )
            Box(Modifier.weight(1f)) {
                Slider(
                    value = scrubbing ?: state.position.toFloat().coerceIn(0f, duration),
                    onValueChange = { scrubbing = it },
                    onValueChangeFinished = {
                        scrubbing?.let { host.seek(it.toDouble()) }
                        scrubbing = null
                    },
                    valueRange = 0f..duration,
                    track = {
                        AccentTrack(
                            it,
                            buffered = if (state.duration > 0) (state.cachedUntil / state.duration).toFloat() else 0f,
                        )
                    },
                )
                ChapterMarks(chapters, state.duration, Modifier.matchParentSize())
            }
            Text(formatTime(state.duration), color = Color.White, style = MaterialTheme.typography.labelMedium)
        }
        Row(
            Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                CompactIcon(Icons.Default.SkipPrevious, "Previous episode", enabled = hasPrevious, onClick = onPrevious)
                CompactIcon(Icons.Default.Replay10, "Back ten seconds") { host.seek(state.position - 10) }
                IconButton(onClick = { host.setPaused(!state.paused) }) {
                    Icon(
                        if (state.paused) Icons.Default.PlayArrow else Icons.Default.Pause,
                        contentDescription = if (state.paused) "Play" else "Pause",
                        tint = Color.White,
                        modifier = Modifier.size(32.dp),
                    )
                }
                CompactIcon(Icons.Default.Forward30, "Forward thirty seconds") { host.seek(state.position + 30) }
                CompactIcon(Icons.Default.SkipNext, "Next episode", enabled = hasNext, onClick = onNext)
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (!roomForAll) {
                    CompactIcon(Icons.Default.Subtitles, "Subtitles") { choosing = "sub" }
                    var more by remember { mutableStateOf(false) }
                    Box {
                        CompactIcon(Icons.Default.MoreVert, "More") { more = true }
                        DropdownMenu(expanded = more, onDismissRequest = { more = false }) {
                            val entries = buildList {
                                if (episodes.size > 1) add(Triple(Icons.Default.VideoLibrary, "Episodes") { showEpisodes = true })
                                if (chapters.size > 1) add(Triple(Icons.AutoMirrored.Filled.Toc, "Chapters") { showChapters = true })
                                add(Triple(Icons.Default.Speed, "Playback speed") { showSpeeds = true })
                                add(Triple(Icons.Default.Audiotrack, "Audio track") { choosing = "audio" })
                                add(
                                    Triple(
                                        if (state.muted) Icons.AutoMirrored.Filled.VolumeUp else Icons.AutoMirrored.Filled.VolumeOff,
                                        if (state.muted) "Unmute" else "Mute",
                                        toggleMute,
                                    ),
                                )
                                add(
                                    Triple(
                                        if (landscapeLocked) Icons.Default.FullscreenExit else Icons.Default.Fullscreen,
                                        if (landscapeLocked) "Unlock rotation" else "Lock to landscape",
                                        onRotate,
                                    ),
                                )
                            }
                            entries.forEach { (icon, label, action) ->
                                DropdownMenuItem(
                                    text = { Text(label) },
                                    leadingIcon = { Icon(icon, contentDescription = null) },
                                    onClick = { more = false; action() },
                                )
                            }
                        }
                    }
                } else {
                    if (episodes.size > 1) {
                        CompactIcon(Icons.Default.VideoLibrary, "Episodes") { showEpisodes = true }
                    }
                    if (chapters.size > 1) {
                        CompactIcon(Icons.AutoMirrored.Filled.Toc, "Chapters") { showChapters = true }
                    }
                    CompactIcon(Icons.Default.Speed, "Playback speed") { showSpeeds = true }
                    CompactIcon(Icons.Default.Audiotrack, "Audio track") { choosing = "audio" }
                    CompactIcon(Icons.Default.Subtitles, "Subtitles") { choosing = "sub" }
                    CompactIcon(
                        if (state.muted) Icons.AutoMirrored.Filled.VolumeOff else Icons.AutoMirrored.Filled.VolumeUp,
                        if (state.muted) "Unmute" else "Mute",
                        onClick = toggleMute,
                    )
                    // The app's own volume, which is not the device's: a file
                    // mastered quiet is turned up here and stays up for the next
                    // one. Only where there is room for it — a phone in portrait
                    // has the system volume keys and no width to spare.
                    if (roomForVolume) {
                        var dragging by remember { mutableStateOf<Float?>(null) }
                        Slider(
                            value = dragging ?: state.volume.toFloat().coerceIn(0f, MAX_VOLUME),
                            onValueChange = { dragging = it; host.setVolume(it.toDouble()) },
                            onValueChangeFinished = {
                                val chosen = dragging?.toDouble()
                                dragging = null
                                chosen?.let { scope.launch(Dispatchers.IO) { rememberVolume(volume = it) } }
                            },
                            valueRange = 0f..MAX_VOLUME,
                            modifier = Modifier.width(96.dp),
                        )
                    }
                    CompactIcon(
                        if (landscapeLocked) Icons.Default.FullscreenExit else Icons.Default.Fullscreen,
                        if (landscapeLocked) "Unlock rotation" else "Lock to landscape",
                        onClick = onRotate,
                    )
                }
            }
        }
    }

    if (showSpeeds) {
        SpeedChooser(
            current = speed,
            onDismiss = { showSpeeds = false },
            onSelect = { chosen ->
                speed = chosen
                host.setSpeed(chosen)
                showSpeeds = false
                scope.launch(Dispatchers.IO) { rememberSpeed(chosen) }
            },
        )
    }
    if (showEpisodes) {
        EpisodeChooser(
            episodes = episodes,
            playing = playing,
            onDismiss = { showEpisodes = false },
            onSelect = { showEpisodes = false; onEpisode(it) },
        )
    }
    if (showChapters) {
        ChapterChooser(
            chapters = chapters,
            position = state.position,
            onDismiss = { showChapters = false },
            onSelect = { host.seek(it.start); showChapters = false },
        )
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
                    // No per-title choice yet: start from what plays everywhere
                    // else, so choosing an audio track does not silently drop
                    // the subtitle preference this episode started with.
                    val global = engine.playbackPrefs()
                    val old = engine.titleTrackPreferences(titleKey) ?: TrackPreferencesRecord(
                        audioLanguage = global.audioLanguage,
                        subtitleLanguage = global.subtitleLanguage,
                        subtitles = global.subtitles,
                    )
                    engine.setTitleTrackPreferences(
                        titleKey,
                        if (type == "audio") {
                            old.copy(audioLanguage = selected?.language?.takeIf(String::isNotBlank))
                        } else {
                            // Turning subtitles off keeps the language it was
                            // off *from*, so turning them back on does not have
                            // to be told the language again — as on desktop.
                            old.copy(
                                subtitleLanguage = selected?.language?.takeIf(String::isNotBlank)
                                    ?: old.subtitleLanguage,
                                subtitles = selected != null,
                            )
                        },
                    )
                }
            },
        )
    }
}

/**
 * Persist a volume or a mute, leaving whichever was not given as it was.
 *
 * Written when the thumb is released rather than while it moves: a drag emits
 * dozens of values a second and every one of them would be a file rewrite. A
 * failure is dropped — the volume is already applied, and the cost of not
 * storing it is that the next launch starts where the last one was stored.
 */
private suspend fun rememberVolume(volume: Double? = null, muted: Boolean? = null) {
    runCatching {
        val engine = NativeRuntime.engine()
        val prefs = engine.playbackPrefs()
        engine.setPlaybackPrefs(prefs.copy(volume = volume ?: prefs.volume, muted = muted ?: prefs.muted))
    }
}

/** Persist a playback rate, so the next episode starts at it. */
private suspend fun rememberSpeed(speed: Double) {
    runCatching {
        val engine = NativeRuntime.engine()
        engine.setPlaybackPrefs(engine.playbackPrefs().copy(speed = speed))
    }
}

/**
 * The rates worth offering.
 *
 * A short list rather than a slider: the useful rates are a handful of ratios,
 * and a slider on a phone lands on 1.07 as often as on 1.0.
 */
private val SPEEDS = listOf(0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0)

@Composable
private fun SpeedChooser(current: Double, onDismiss: () -> Unit, onSelect: (Double) -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Playback speed") },
        text = {
            Column(Modifier.fillMaxWidth()) {
                SPEEDS.forEach { rate ->
                    EdgedButton(
                        onClick = { onSelect(rate) },
                        modifier = Modifier.fillMaxWidth().padding(vertical = 2.dp),
                    ) {
                        Text(
                            (if (kotlin.math.abs(rate - current) < 0.001) "✓  " else "") +
                                if (rate == 1.0) "Normal" else "${rate}×",
                        )
                    }
                }
            }
        },
        confirmButton = { AccentButton(onClick = onDismiss) { Text("Close") } },
    )
}

/** Where each chapter begins, ticked onto the seek bar. */
@Composable
private fun ChapterMarks(chapters: List<ChapterEntry>, duration: Double, modifier: Modifier) {
    if (chapters.size < 2 || duration <= 0.0) return
    Canvas(modifier.padding(horizontal = 10.dp)) {
        val width = 2.dp.toPx()
        val height = 8.dp.toPx()
        chapters.drop(1).forEach { chapter ->
            val fraction = (chapter.start / duration).coerceIn(0.0, 1.0).toFloat()
            drawRect(
                color = Color.White.copy(alpha = 0.75f),
                topLeft = Offset(fraction * size.width - width / 2, (size.height - height) / 2),
                size = Size(width, height),
            )
        }
    }
}

@Composable
private fun ChapterChooser(
    chapters: List<ChapterEntry>,
    position: Double,
    onDismiss: () -> Unit,
    onSelect: (ChapterEntry) -> Unit,
) {
    val current = chapters.indexOfLast { position + 0.001 >= it.start }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Chapters") },
        text = {
            LazyColumn(Modifier.fillMaxWidth().heightIn(max = 420.dp)) {
                items(chapters, key = ChapterEntry::index) { chapter ->
                    EdgedButton(
                        onClick = { onSelect(chapter) },
                        modifier = Modifier.fillMaxWidth().padding(vertical = 2.dp),
                    ) {
                        Text(
                            (if (chapters.getOrNull(current) === chapter) "✓  " else "") +
                                "${formatTime(chapter.start)}  ${chapter.label}",
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
            }
        },
        confirmButton = { AccentButton(onClick = onDismiss) { Text("Close") } },
    )
}

/**
 * The title's episodes, from the player: a sheet rather than a dialog, opened
 * scrolled to the one playing so the next few are in view without a search.
 * Each row has its still, its name, and whether it is playing, watched or
 * part way.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun EpisodeChooser(
    episodes: List<EpisodeRecord>,
    playing: Int,
    onDismiss: () -> Unit,
    onSelect: (Int) -> Unit,
) {
    val list = rememberLazyListState(initialFirstVisibleItemIndex = (playing - 1).coerceAtLeast(0))
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Text(
            "Episodes",
            style = MaterialTheme.typography.titleMedium,
            modifier = Modifier.padding(horizontal = 24.dp, vertical = 8.dp),
        )
        LazyColumn(state = list, contentPadding = PaddingValues(bottom = 24.dp)) {
            itemsIndexed(episodes, key = { _, it -> it.linkId }) { position, episode ->
                val current = position == playing
                Row(
                    Modifier
                        .fillMaxWidth()
                        .clickable { onSelect(position) }
                        .background(
                            if (current) MaterialTheme.colorScheme.secondaryContainer else Color.Transparent,
                        )
                        .padding(horizontal = 16.dp, vertical = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Box(Modifier.width(112.dp).aspectRatio(16f / 9f).clip(MaterialTheme.shapes.small)) {
                        RemoteArtwork(
                            episode.stillUrl,
                            episode.label,
                            Modifier.fillMaxSize(),
                            fallback = episode.thumbnailSource,
                            labelled = false,
                        )
                        episode.progress?.takeIf { !episode.watched && !current }?.let { progress ->
                            AccentProgress(
                                progress = { progress.toFloat().coerceIn(0f, 1f) },
                                modifier = Modifier.fillMaxWidth().align(Alignment.BottomCenter),
                            )
                        }
                    }
                    Column(Modifier.weight(1f).padding(start = 12.dp)) {
                        Text(
                            episode.providerName ?: episode.label,
                            style = MaterialTheme.typography.titleSmall,
                            maxLines = 2,
                            overflow = TextOverflow.Ellipsis,
                        )
                        Text(
                            when {
                                current -> "Playing"
                                episode.watched -> "${episode.label} · Watched"
                                episode.resumeAt != null -> "${episode.label} · Part way"
                                else -> episode.label
                            },
                            style = MaterialTheme.typography.bodySmall,
                            color = if (current) {
                                MaterialTheme.colorScheme.primary
                            } else {
                                MaterialTheme.colorScheme.onSurfaceVariant
                            },
                        )
                    }
                }
            }
        }
    }
}

/**
 * The countdown into the next episode, shown once the credits run starts.
 *
 * "Watch till the end" holds for the rest of the file: someone who wants the
 * post-credits scene should have to say so once, not once every ten seconds.
 *
 * With [secondsLeft] the card counts down the end of a file that has no credits
 * chapter, on the file's own clock; the end of file is what advances, and the
 * second button is "Cancel", since there are no credits to watch.
 */
@Composable
private fun UpNextCard(
    next: EpisodeRecord?,
    visible: Boolean,
    secondsLeft: Int?,
    onPlayNow: () -> Unit,
    onStay: () -> Unit,
    modifier: Modifier = Modifier,
) {
    var held by remember(next?.linkId) { mutableStateOf(false) }
    var counted by remember(next?.linkId) { mutableStateOf(UP_NEXT_SECONDS) }
    val remaining = secondsLeft ?: counted
    val upNext = next
    val showing = visible && upNext != null && !held
    LaunchedEffect(showing, secondsLeft == null) {
        if (!showing || secondsLeft != null) return@LaunchedEffect
        counted = UP_NEXT_SECONDS
        while (counted > 0) {
            delay(1_000)
            counted -= 1
        }
        onPlayNow()
    }
    // `showing` already implies a non-null episode, and the compiler knows it.
    if (!showing) return
    // Eased between ticks, so the ring runs down rather than stepping once a
    // second.
    val ring by animateFloatAsState(
        targetValue = remaining.toFloat() / UP_NEXT_SECONDS,
        animationSpec = tween(durationMillis = 1_000, easing = LinearEasing),
        label = "up-next countdown",
    )
    Column(
        modifier
            .width(280.dp)
            .background(MaterialTheme.colorScheme.surfaceContainerHighest, MaterialTheme.shapes.medium)
            .padding(12.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            // The next episode's still, so the card says which episode
            // without being read; the ring over it is both the countdown and
            // the button that skips it.
            Box(
                Modifier
                    .width(112.dp)
                    .aspectRatio(16f / 9f)
                    .clip(MaterialTheme.shapes.small)
                    .clickable(onClick = onPlayNow),
                contentAlignment = Alignment.Center,
            ) {
                RemoteArtwork(
                    upNext.stillUrl,
                    upNext.label,
                    Modifier.fillMaxSize(),
                    fallback = upNext.thumbnailSource,
                    labelled = false,
                )
                Box(Modifier.size(40.dp).clip(CircleShape).background(Color.Black.copy(alpha = 0.5f)))
                CircularProgressIndicator(
                    progress = { ring },
                    modifier = Modifier.size(40.dp),
                    color = Color.White,
                    trackColor = Color.White.copy(alpha = 0.25f),
                    strokeWidth = 3.dp,
                )
                Icon(Icons.Default.PlayArrow, contentDescription = "Play now", tint = Color.White)
            }
            Column(Modifier.weight(1f).padding(start = 12.dp)) {
                Text(
                    "Up next in ${remaining}s",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.labelMedium,
                )
                Text(
                    upNext.providerName ?: upNext.label,
                    color = MaterialTheme.colorScheme.onSurface,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                    style = MaterialTheme.typography.titleSmall,
                )
            }
        }
        if (secondsLeft != null) {
            QuietButton(onClick = { held = true; onStay() }, modifier = Modifier.padding(top = 4.dp)) {
                Text("Cancel")
            }
        } else {
            QuietButton(onClick = { held = true }, modifier = Modifier.padding(top = 4.dp)) {
                Text("Watch till the end")
            }
        }
    }
}

/** What the player says once an episode ends and nothing starts after it. */
@Composable
private fun EndCard(
    finished: String,
    show: String,
    next: EpisodeRecord?,
    onPlayNext: () -> Unit,
    onReplay: () -> Unit,
    onClose: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(
        modifier
            .widthIn(max = 360.dp)
            .background(MaterialTheme.colorScheme.surfaceContainerHighest, MaterialTheme.shapes.medium)
            .padding(20.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(
            if (next == null) "That was the last episode of $show" else "Finished $finished",
            color = MaterialTheme.colorScheme.onSurface,
            style = MaterialTheme.typography.titleMedium,
            textAlign = TextAlign.Center,
        )
        Row(Modifier.padding(top = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            if (next != null) {
                AccentButton(onClick = onPlayNext) { Text("Play next") }
            } else {
                QuietButton(onClick = onReplay) { Text("Watch again") }
            }
            QuietButton(onClick = onClose, modifier = Modifier.padding(start = 8.dp)) { Text("Close") }
        }
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
                        EdgedButton(onClick = { onSelect(null) }, Modifier.fillMaxWidth()) {
                            Text(if (tracks.none(MpvTrack::selected)) "✓  Off" else "Off")
                        }
                    }
                }
                items(tracks, key = MpvTrack::id) { track ->
                    EdgedButton(onClick = { onSelect(track) }, Modifier.fillMaxWidth()) {
                        Text(if (track.selected) "✓  ${track.label}" else track.label)
                    }
                }
            }
        },
        confirmButton = { AccentButton(onClick = onDismiss) { Text("Close") } },
    )
}

@Composable
private fun CompactIcon(
    icon: androidx.compose.ui.graphics.vector.ImageVector,
    description: String,
    enabled: Boolean = true,
    onClick: () -> Unit,
) {
    // 48dp: the smallest target Android's accessibility guidance allows, and
    // the size a thumb on a moving train actually hits.
    IconButton(onClick = onClick, enabled = enabled, modifier = Modifier.size(48.dp)) {
        Icon(
            icon,
            contentDescription = description,
            tint = if (enabled) Color.White else Color.White.copy(alpha = 0.35f),
            modifier = Modifier.size(24.dp),
        )
    }
}

private fun ((Double) -> Unit).asFloat(): (Float) -> Unit = { this(it.toDouble()) }

/** How much of the episode is left, or nothing before the duration is known. */
private fun remaining(state: MpvPlaybackState): String? = (state.duration - state.position)
    .takeIf { state.duration > 0.0 && it >= 1.0 }
    ?.let { "${formatTime(it)} left" }

private fun formatTime(seconds: Double): String {
    val total = seconds.coerceAtLeast(0.0).roundToInt()
    val hours = total / 3_600
    return if (hours > 0) "%d:%02d:%02d".format(hours, (total % 3_600) / 60, total % 60)
    else "%d:%02d".format(total / 60, total % 60)
}
