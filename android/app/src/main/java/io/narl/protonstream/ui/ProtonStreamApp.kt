package io.narl.protonstream.ui

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.ExperimentalSharedTransitionApi
import androidx.compose.animation.SharedTransitionLayout
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.material3.VerticalDivider
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.calculateEndPadding
import androidx.compose.foundation.layout.calculateStartPadding
import androidx.compose.material3.Card
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.ScaffoldDefaults
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.plus
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Text
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.VideoLibrary
import androidx.compose.material.icons.filled.Share
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.History
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material3.adaptive.navigationsuite.NavigationSuiteScaffold
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.work.WorkManager
import io.narl.protonstream.ui.theme.accentNavigationColors
import uniffi.pstr_android.TitleRecord
import uniffi.pstr_android.EpisodeRecord
import uniffi.pstr_android.SeasonRecord
import io.narl.protonstream.playback.LibmpvHost
import io.narl.protonstream.playback.NativeMpvHost
import io.narl.protonstream.playback.PlayerScreen

/** From this width the library and an open title share the window. */
private val TWO_PANE_WIDTH = 840.dp

/** How much room the floating mini transport needs at the foot of a page. */
private val MINI_TRANSPORT_INSET = 88.dp

/** What the player was asked to open: a title's episodes, and which one. */
private data class PlayRequest(
    val title: TitleRecord,
    val episodes: List<EpisodeRecord>,
    val index: Int,
)

private enum class Destination(val label: String, val icon: ImageVector) {
    Library("Library", Icons.Default.VideoLibrary),
    History("History", Icons.Default.History),
    Shares("Shares", Icons.Default.Share),
    Downloads("Downloads", Icons.Default.Download),
    Settings("Settings", Icons.Default.Settings),
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalSharedTransitionApi::class)
@Composable
fun ProtonStreamApp(
    playerHost: LibmpvHost? = null,
    inPictureInPicture: Boolean = false,
    shareLink: String? = null,
    onShareLinkTaken: () -> Unit = {},
    model: AppViewModel = viewModel(
        factory = AppViewModel.Factory(
            LocalContext.current,
            WorkManager.getInstance(LocalContext.current),
        ),
    ),
) {
    val state by model.state.collectAsState()
    val snackbars = remember { SnackbarHostState() }
    // Keys, not records: what has to survive process recreation is *which* title
    // and which episode, and a `TitleRecord` is neither parcelable nor still
    // current after the library reloads. Everything else is derived, so a reload
    // that renames a season updates the open screen rather than pinning a stale
    // copy of it.
    var destination by rememberSaveable { mutableStateOf(Destination.Library) }
    var selectedTitleKey by rememberSaveable { mutableStateOf<String?>(null) }
    var playingTitleKey by rememberSaveable { mutableStateOf<String?>(null) }
    var playingIndex by rememberSaveable { mutableIntStateOf(0) }
    // Playing, but not on screen: the episode keeps going and the mini transport
    // is what says so. Saved, because a rotation must not throw the viewer back
    // into the video they had just left.
    var playerMinimized by rememberSaveable { mutableStateOf(false) }

    val selectedTitle = state.titles.firstOrNull { it.key == selectedTitleKey }
    val playing = playingTitleKey
        ?.let { key -> state.titles.firstOrNull { it.key == key } }
        ?.let { title ->
            val episodes = title.seasons.flatMap(SeasonRecord::episodes)
            PlayRequest(title, episodes, playingIndex.coerceIn(0, (episodes.size - 1).coerceAtLeast(0)))
        }

    // A link opened from a browser or shared from another app: show the Add
    // form with it filled in. A playing episode keeps going in the mini
    // transport rather than covering the form.
    LaunchedEffect(shareLink) {
        if (shareLink != null) {
            destination = Destination.Shares
            selectedTitleKey = null
            if (playingTitleKey != null) playerMinimized = true
        }
    }
    LaunchedEffect(state.message) {
        state.message?.let {
            val undoable = state.undo != null
            val result = snackbars.showSnackbar(
                it,
                actionLabel = if (undoable) "Undo" else null,
                duration = if (undoable) SnackbarDuration.Long else SnackbarDuration.Short,
            )
            if (result == SnackbarResult.ActionPerformed) model.undo() else model.dismissMessage()
        }
    }
    // Read out here because `navigationSuiteItems` is not a composable scope.
    val navigation = accentNavigationColors()
    NavigationSuiteScaffold(
        navigationSuiteItems = {
            Destination.entries.forEach { item ->
                item(
                    selected = destination == item,
                    onClick = { destination = item; selectedTitleKey = null },
                    icon = { Icon(item.icon, contentDescription = item.label) },
                    label = { Text(item.label) },
                    colors = navigation,
                )
            }
        },
    ) {
        Scaffold(
            snackbarHost = { SnackbarHost(snackbars) },
            // Each page draws its own bar under the status bar, and the title
            // page draws its backdrop there, so the top inset is theirs.
            contentWindowInsets = ScaffoldDefaults.contentWindowInsets
                .only(WindowInsetsSides.Horizontal + WindowInsetsSides.Bottom),
        ) { padding ->
            // Back from a secondary tab returns to the library rather than
            // leaving the app; the library's own back is the system's, which is
            // what a top-level destination should do.
            BackHandler(enabled = destination != Destination.Library) {
                destination = Destination.Library
            }
            val minimizedHost = (playerHost as? NativeMpvHost)?.takeIf { playing != null && playerMinimized }
            // The mini transport floats over the page, so the page has to end
            // above it. Without this the last episode of a season sits under the
            // bar and cannot be scrolled clear of it.
            val direction = LocalLayoutDirection.current
            val body = if (minimizedHost == null) padding else PaddingValues(
                start = padding.calculateStartPadding(direction),
                end = padding.calculateEndPadding(direction),
                top = padding.calculateTopPadding(),
                bottom = padding.calculateBottomPadding() + MINI_TRANSPORT_INSET,
            )
            Box(Modifier.fillMaxSize()) {
            AnimatedContent(destination, label = "primary navigation") { target ->
                when (target) {
                    // Side by side where there is room for both: on a tablet the
                    // library stays in view while a title is open, so moving
                    // between titles is one tap rather than back and in again.
                    Destination.Library -> BoxWithConstraints(Modifier.fillMaxSize()) {
                        val libraryPane = @Composable {
                                LibraryScreen(
                                state,
                                model::search,
                                { title, index ->
                                    playingTitleKey = title.key
                                    playingIndex = index
                                    playerMinimized = false
                                },
                                { selectedTitleKey = it.key },
                                model::refresh,
                                model::setTitleWatched,
                                model::forgetPosition,
                                body,
                                onMatchChanged = model::reloadAfterMetadataChange,
                                onError = model::reportError,
                                onArrange = model::arrange,
                                )
                        }
                        val titlePane = @Composable { selected: TitleRecord ->
                                TitleScreen(
                                selected,
                                playerHost != null,
                                { _, index ->
                                    playingTitleKey = selected.key
                                    playingIndex = index
                                    playerMinimized = false
                                },
                                { selectedTitleKey = null },
                                model::reportError,
                                model::reloadAfterMetadataChange,
                                model::setWatched,
                                body,
                                franchise = selected.franchise.mapNotNull { key ->
                                    state.titles.firstOrNull { it.key == key }
                                },
                                onOpenTitle = { selectedTitleKey = it.key },
                                )
                        }
                        if (maxWidth >= TWO_PANE_WIDTH) {
                            Row(Modifier.fillMaxSize()) {
                                Box(Modifier.weight(0.45f)) { libraryPane() }
                                VerticalDivider()
                                Box(Modifier.weight(0.55f)) {
                                    selectedTitle?.let { titlePane(it) } ?: Box(Modifier.padding(body)) {
                                        EmptyState("Choose a title", "Its seasons and episodes open here.")
                                    }
                                }
                            }
                        } else {
                            SharedTransitionLayout {
                                AnimatedContent(selectedTitle, contentKey = { it?.key }, label = "title") { open ->
                                    CompositionLocalProvider(
                                        LocalSharedTransition provides this@SharedTransitionLayout,
                                        LocalPaneVisibility provides this,
                                    ) {
                                        open?.let { titlePane(it) } ?: libraryPane()
                                    }
                                }
                            }
                        }
                    }
                    Destination.Shares -> SharesScreen(
                        state.shares,
                        model::addShare,
                        model::repairShare,
                        model::refreshShare,
                        model::removeShare,
                        shareLink,
                        onShareLinkTaken,
                        body,
                    )
                    Destination.History -> HistoryScreen(
                        state,
                        { title, index ->
                            playingTitleKey = title.key
                            playingIndex = index
                            playerMinimized = false
                        },
                        { destination = Destination.Library; selectedTitleKey = it.key },
                        model::removeFromHistory,
                        body,
                    )
                    Destination.Downloads -> DownloadsScreen(
                        state,
                        model::removeOffline,
                        model::pauseDownload,
                        model::resumeDownload,
                        model::deletePartial,
                        body,
                    )
                    Destination.Settings -> SettingsScreen(
                        state,
                        model::saveMetadataSettings,
                        { model.matchTitles(force = true) },
                        model::clearBlockCache,
                        model::removeAllOffline,
                        body,
                        onNamesChanged = model::reloadAfterMetadataChange,
                    )
                }
            }
            minimizedHost?.let { core ->
                MiniTransport(
                    host = core,
                    onRestore = { playerMinimized = false },
                    onClose = {
                        playingTitleKey = null
                        playerMinimized = false
                        model.reloadAfterMetadataChange()
                    },
                    modifier = Modifier
                        .align(Alignment.BottomCenter)
                        .padding(bottom = padding.calculateBottomPadding() + 8.dp),
                )
            }
            }
        }
    }

    // Drawn over the scaffold rather than instead of it. The player owns the
    // whole window while it is up — it is a screen, not a dialog over one, which
    // is what lets it draw into the cutout and hand the right window to
    // Picture-in-Picture — but the page behind it stays composed, so leaving the
    // player finds the title page where it was rather than rebuilt from the top.
    playing?.let { request ->
        PlayerScreen(
            title = request.title,
            episodes = request.episodes,
            index = request.index,
            host = playerHost,
            inPictureInPicture = inPictureInPicture,
            minimized = playerMinimized,
            onIndexChange = { playingIndex = it },
            onSaveProgress = model::saveProgress,
            onMinimize = { playerMinimized = true },
            onClose = {
                playingTitleKey = null
                playerMinimized = false
                model.reloadAfterMetadataChange()
            },
        )
    }
}

/**
 * The bar that says playback is still going, and gets back to it.
 *
 * Leaving the player does not stop it — the episode keeps playing, which is the
 * whole point of the background-audio setting — so there has to be something on
 * screen that says so and takes one tap to return to. Without it, the only way
 * back to a playing episode is to find it in the library again.
 */
@Composable
private fun MiniTransport(
    host: NativeMpvHost,
    onRestore: () -> Unit,
    onClose: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val state by host.state.collectAsState()
    val playing by host.nowPlaying.collectAsState()
    val episode = playing ?: return
    Card(modifier.fillMaxWidth().padding(horizontal = 8.dp).clickable(onClick = onRestore)) {
        Row(Modifier.padding(8.dp), verticalAlignment = Alignment.CenterVertically) {
            RemoteArtwork(
                episode.artworkUrl,
                episode.show ?: episode.title,
                Modifier.width(64.dp).aspectRatio(16f / 9f).clip(MaterialTheme.shapes.small),
                fallback = episode.artworkFile,
            )
            Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                Text(episode.title, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                episode.show?.let {
                    Text(it, style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            }
            IconButton(onClick = { host.setPaused(!state.paused) }) {
                Icon(
                    if (state.paused) Icons.Default.PlayArrow else Icons.Default.Pause,
                    contentDescription = if (state.paused) "Play" else "Pause",
                )
            }
            IconButton(onClick = onClose) {
                Icon(Icons.Default.Close, contentDescription = "Stop playback")
            }
        }
    }
}
