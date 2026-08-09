package io.narl.protonstream.ui

import android.content.Intent
import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.foundation.clickable
import androidx.compose.foundation.border
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.calculateEndPadding
import androidx.compose.foundation.layout.calculateStartPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.filled.VideoLibrary
import androidx.compose.material.icons.filled.Share
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material.icons.filled.ExpandLess
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.adaptive.navigationsuite.NavigationSuiteScaffold
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.livedata.observeAsState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.window.DialogProperties
import androidx.compose.ui.window.SecureFlagPolicy
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.net.toUri
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.work.WorkManager
import io.narl.protonstream.settings.SettingsStore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import io.narl.protonstream.download.DownloadCoordinator
import io.narl.protonstream.download.DownloadStateStore
import io.narl.protonstream.download.RetainedDownload
import io.narl.protonstream.native.NativeRuntime
import io.narl.protonstream.ui.theme.AccentButton
import io.narl.protonstream.ui.theme.AccentProgress
import io.narl.protonstream.ui.theme.AppearanceState
import io.narl.protonstream.ui.theme.EdgedButton
import io.narl.protonstream.ui.theme.QuietButton
import io.narl.protonstream.ui.theme.TonalButton
import io.narl.protonstream.ui.theme.accentNavigationColors
import io.narl.protonstream.ui.theme.solid
import uniffi.pstr_android.ShareRecord
import uniffi.pstr_android.TitleRecord
import uniffi.pstr_android.TrackPreferencesRecord
import uniffi.pstr_android.EpisodeRecord
import uniffi.pstr_android.SeasonRecord
import uniffi.pstr_android.MatchRecord
import uniffi.pstr_android.MetadataProvider
import uniffi.pstr_android.FlavorChoice
import uniffi.pstr_android.AppearanceRecord
import uniffi.pstr_android.AccentChoice
import uniffi.pstr_android.PlaybackPrefsRecord
import io.narl.protonstream.playback.LibmpvHost
import io.narl.protonstream.playback.NativeMpvHost
import io.narl.protonstream.playback.PlayerScreen

/**
 * How many shows the continue-watching shelf holds.
 *
 * A shelf is a shortcut, not a second library: past about this many it is
 * quicker to search for the thing than to scroll the shelf looking for it.
 */
private const val CONTINUE_WATCHING_MAX = 12

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
    Shares("Shares", Icons.Default.Share),
    Downloads("Downloads", Icons.Default.Download),
    Settings("Settings", Icons.Default.Settings),
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ProtonStreamApp(
    playerHost: LibmpvHost? = null,
    inPictureInPicture: Boolean = false,
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

    LaunchedEffect(state.message) {
        state.message?.let {
            snackbars.showSnackbar(it)
            model.dismissMessage()
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
            topBar = {
                TopAppBar(
                    title = { Text(selectedTitle?.name ?: "proton-stream") },
                    actions = {
                        if (destination == Destination.Library) {
                            if (state.refreshing) {
                                CircularProgressIndicator(modifier = Modifier.size(24.dp))
                                Spacer(Modifier.width(16.dp))
                            } else {
                                IconButton(onClick = model::refresh) {
                                    Icon(Icons.Default.Refresh, contentDescription = "Refresh")
                                }
                            }
                        }
                    },
                )
            },
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
                    Destination.Library -> if (selectedTitle == null) {
                        LibraryScreen(
                            state,
                            model::search,
                            { title, index ->
                                playingTitleKey = title.key
                                playingIndex = index
                                playerMinimized = false
                            },
                            { selectedTitleKey = it.key },
                            body,
                        )
                    } else {
                        TitleScreen(
                            selectedTitle,
                            playerHost != null,
                            { _, index ->
                                playingTitleKey = selectedTitle.key
                                playingIndex = index
                                playerMinimized = false
                            },
                            { selectedTitleKey = null },
                            model::reportError,
                            model::reloadAfterMetadataChange,
                            model::setWatched,
                            body,
                        )
                    }
                    Destination.Shares -> SharesScreen(
                        state.shares,
                        model::addShare,
                        model::repairShare,
                        model::refreshShare,
                        model::removeShare,
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

/** One row of the continue-watching shelf: which episode, and where it sits. */
private data class Resumable(val title: TitleRecord, val episode: EpisodeRecord, val index: Int)

@Composable
private fun LibraryScreen(
    state: AppUiState,
    onSearch: (String) -> Unit,
    onResume: (TitleRecord, Int) -> Unit,
    onTitle: (TitleRecord) -> Unit,
    padding: PaddingValues,
) {
    // Most recently played first, one episode per show: a shelf that lists four
    // episodes of the same series is a shelf with room for nothing else.
    val resumable = remember(state.titles) {
        state.titles.mapNotNull { title ->
            val playlist = title.seasons.flatMap(SeasonRecord::episodes)
            playlist.withIndex()
                .filter { it.value.resumeAt != null && !it.value.watched }
                .maxByOrNull { it.value.lastPlayed }
                ?.let { Resumable(title, it.value, it.index) }
        }.sortedByDescending { it.episode.lastPlayed }.take(CONTINUE_WATCHING_MAX)
    }
    Column(Modifier.fillMaxSize().padding(padding).padding(horizontal = 16.dp)) {
        OutlinedTextField(
            value = state.query,
            onValueChange = onSearch,
            modifier = Modifier.fillMaxWidth(),
            singleLine = true,
            placeholder = { Text("Search library") },
            leadingIcon = { Icon(Icons.Default.Search, contentDescription = null) }
        )
        Spacer(Modifier.height(16.dp))
        if (state.loading) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
        } else if (state.titles.isEmpty()) {
            EmptyState("Your library is empty", "Add a Proton Drive public link under Shares, then refresh.")
        } else {
            LazyVerticalGrid(
                columns = GridCells.Adaptive(180.dp),
                horizontalArrangement = Arrangement.spacedBy(12.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
                contentPadding = PaddingValues(bottom = 24.dp),
            ) {
                // Above the grid, and only when there is something in it: what a
                // viewer opening the app wants is almost always the thing they
                // were part way through, and finding it in an alphabetical grid
                // is the long way round.
                //
                // Inside the grid rather than pinned over it, so the shelf
                // scrolls away with the rest instead of holding a fixed slice of
                // a phone screen for itself.
                if (state.query.isBlank() && resumable.isNotEmpty()) {
                    item(key = "continue-watching", span = { GridItemSpan(maxLineSpan) }) {
                        Column {
                            Text("Continue watching", style = MaterialTheme.typography.titleMedium)
                            LazyRow(
                                horizontalArrangement = Arrangement.spacedBy(12.dp),
                                contentPadding = PaddingValues(vertical = 12.dp),
                            ) {
                                items(
                                    resumable,
                                    key = { "${it.episode.shareId}/${it.episode.linkId}" },
                                ) { entry ->
                                    Card(
                                        Modifier.width(200.dp)
                                            .clickable { onResume(entry.title, entry.index) },
                                    ) {
                                        RemoteArtwork(
                                            entry.episode.stillUrl ?: entry.title.backdropUrl,
                                            entry.title.canonicalName ?: entry.title.name,
                                            Modifier.fillMaxWidth().aspectRatio(16f / 9f),
                                            fallback = entry.episode.thumbnailSource,
                                        )
                                        Column(Modifier.padding(10.dp)) {
                                            Text(
                                                entry.title.canonicalName ?: entry.title.name,
                                                fontWeight = FontWeight.SemiBold,
                                                maxLines = 1,
                                                overflow = TextOverflow.Ellipsis,
                                            )
                                            Text(
                                                entry.episode.label,
                                                style = MaterialTheme.typography.bodySmall,
                                                maxLines = 1,
                                                overflow = TextOverflow.Ellipsis,
                                            )
                                            entry.episode.progress?.let { progress ->
                                                AccentProgress(
                                                    progress = {
                                                        progress.toFloat().coerceIn(0f, 1f)
                                                    },
                                                    modifier = Modifier.fillMaxWidth()
                                                        .padding(top = 8.dp),
                                                )
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                items(state.titles, key = { it.key }) { title ->
                    Card(Modifier.fillMaxWidth().clickable { onTitle(title) }) {
                        RemoteArtwork(
                            title.backdropUrl ?: title.posterUrl,
                            title.canonicalName ?: title.name,
                            Modifier.fillMaxWidth().height(210.dp),
                            fallback = title.thumbnailSource,
                        )
                        Column(Modifier.padding(12.dp)) {
                            Text(title.name, fontWeight = FontWeight.SemiBold, maxLines = 2, overflow = TextOverflow.Ellipsis)
                            Text(
                                listOfNotNull(title.year?.toString(), "${title.episodeCount} episodes").joinToString(" · "),
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun TitleScreen(
    title: TitleRecord,
    playerReady: Boolean,
    onPlay: (List<EpisodeRecord>, Int) -> Unit,
    onBack: () -> Unit,
    onPreferenceError: (Throwable) -> Unit,
    onMetadataChanged: () -> Unit,
    onSetWatched: (EpisodeRecord, Boolean) -> Unit,
    padding: PaddingValues,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var audioLanguage by remember(title.key) { mutableStateOf("") }
    var subtitleLanguage by remember(title.key) { mutableStateOf("") }
    var subtitlesEnabled by remember(title.key) { mutableStateOf(false) }
    var showMatch by remember(title.key) { mutableStateOf(false) }
    var expandedSeasons by remember(title.key) { mutableStateOf(setOf(title.seasons.firstOrNull()?.label)) }

    BackHandler(onBack = onBack)

    LaunchedEffect(title.key) {
        runCatching {
            withContext(Dispatchers.IO) {
                NativeRuntime.engine().titleTrackPreferences(title.key)
            }
        }.onSuccess { preferences ->
            // Absent means this title has never been given a choice; the fields
            // are then empty and the global preference is what plays.
            audioLanguage = preferences?.audioLanguage.orEmpty()
            subtitleLanguage = preferences?.subtitleLanguage.orEmpty()
            subtitlesEnabled = preferences?.subtitles ?: true
        }.onFailure(onPreferenceError)
    }
    // Display order across seasons: what previous/next and autoplay walk, and
    // the same order the desktop client uses.
    val playlist = remember(title) { title.seasons.flatMap(SeasonRecord::episodes) }
    // Where a press on Play lands. Part-watched wins over unwatched, most
    // recently played first, which is the order the desktop client resumes in.
    val nextUp = remember(playlist) {
        val resumable = playlist.withIndex().filter { it.value.resumeAt != null }
        resumable.maxByOrNull { it.value.lastPlayed }?.index
            ?: playlist.indexOfFirst { !it.watched }.takeIf { it >= 0 }
            ?: 0
    }
    // Live download state per episode, so a row can show progress and be paused
    // without a trip to the Downloads tab.
    val work by WorkManager.getInstance(context)
        .getWorkInfosByTagLiveData(DownloadCoordinator.TAG).observeAsState(emptyList())
    val downloads = remember(work) { DownloadStateStore(context).records() }

    LazyColumn(
        modifier = Modifier.fillMaxSize().padding(padding),
        contentPadding = PaddingValues(16.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        item {
            QuietButton(onClick = onBack) {
                Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = null)
                Spacer(Modifier.width(8.dp))
                Text("Back to library")
            }
            RemoteArtwork(
                title.backdropUrl ?: title.posterUrl,
                title.canonicalName ?: title.name,
                Modifier.fillMaxWidth().aspectRatio(16f / 9f).padding(top = 12.dp),
                fallback = title.thumbnailSource,
            )
            Text(title.canonicalName ?: title.name, style = MaterialTheme.typography.headlineMedium, modifier = Modifier.padding(top = 12.dp))
            title.originalName?.takeIf { it != title.canonicalName }?.let {
                Text(it, style = MaterialTheme.typography.bodyMedium)
            }
            Text(
                listOfNotNull(
                    (title.metadataYear ?: title.year)?.toString(),
                    title.rating?.let { "%.1f/10".format(it) },
                    title.genres.takeIf { it.isNotEmpty() }?.joinToString(" · "),
                ).joinToString("  •  "),
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.padding(top = 6.dp),
            )
            title.overview?.let { Text(it, modifier = Modifier.padding(top = 10.dp)) }
            // Wrapping, not squeezing: a fixed row hands the last button the
            // width the others left over, which is how "More on AniList" ended
            // up set one letter per line on a phone.
            FlowRow(
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.fillMaxWidth().padding(top = 12.dp),
            ) {
                // What a press on the poster should play: whatever was left
                // part-watched, else the first unwatched episode, else the
                // first — the desktop client's `next_up`.
                AccentButton(onClick = { onPlay(playlist, nextUp) }, enabled = playerReady && playlist.isNotEmpty()) {
                    Text(
                        when {
                            !playerReady -> "Player loading…"
                            playlist.getOrNull(nextUp)?.resumeAt != null -> "Resume"
                            title.watchedCount > 0uL -> "Continue"
                            else -> "Play"
                        },
                    )
                }
                // Only where it would do something different: an unstarted show
                // already starts at the beginning.
                if (playlist.getOrNull(nextUp)?.resumeAt != null || title.watchedCount > 0uL) {
                    TonalButton(
                        onClick = { onPlay(playlist, 0) },
                        enabled = playerReady && playlist.isNotEmpty(),
                    ) { Text("Start over") }
                }
                // The rest in the same flow, and all the same kind of button:
                // one filled primary for the thing the page is for, tonal for
                // everything else. A text link among filled buttons was what
                // made this block read as three unrelated rows of controls.
                TonalButton(onClick = { DownloadCoordinator.enqueue(context, playlist) }) {
                    Icon(Icons.Default.Download, contentDescription = null, modifier = Modifier.size(18.dp))
                    Spacer(Modifier.width(8.dp))
                    Text("Download show")
                }
                TonalButton(onClick = { showMatch = true }) {
                    Icon(Icons.Default.Edit, contentDescription = null, modifier = Modifier.size(18.dp))
                    Spacer(Modifier.width(8.dp))
                    Text("Change match")
                }
                // The provider's own page for this title: where a viewer goes to
                // check that the thing the app matched is the thing they have.
                title.externalUrl?.let { url ->
                    EdgedButton(onClick = {
                        runCatching {
                            context.startActivity(Intent(Intent.ACTION_VIEW, url.toUri()))
                        }.onFailure(onPreferenceError)
                    }) { Text("More on ${title.metadataProvider?.displayName() ?: "the provider"}") }
                }
            }
            // Said plainly, because it changes what a re-match will do: a
            // hand-picked match is not overwritten by an automatic pass.
            if (title.manualMatch) {
                Text(
                    "Matched by hand",
                    style = MaterialTheme.typography.labelMedium,
                    modifier = Modifier.padding(top = 8.dp),
                )
            }
            Text("${title.watchedCount} of ${title.episodeCount} watched", Modifier.padding(vertical = 12.dp))
            Text("Preferred tracks", style = MaterialTheme.typography.titleMedium)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(
                    audioLanguage,
                    { audioLanguage = it },
                    label = { Text("Audio language") },
                    modifier = Modifier.weight(1f),
                    singleLine = true,
                )
                OutlinedTextField(
                    subtitleLanguage,
                    { subtitleLanguage = it },
                    label = { Text("Subtitle language") },
                    modifier = Modifier.weight(1f),
                    singleLine = true,
                )
            }
            SettingToggle("Enable subtitles", subtitlesEnabled) { subtitlesEnabled = it }
            TonalButton(onClick = {
                scope.launch {
                    runCatching {
                        withContext(Dispatchers.IO) {
                            NativeRuntime.engine().setTitleTrackPreferences(
                                title.key,
                                TrackPreferencesRecord(
                                    audioLanguage.takeIf(String::isNotBlank),
                                    subtitleLanguage.takeIf(String::isNotBlank),
                                    subtitlesEnabled,
                                ),
                            )
                        }
                    }.onFailure(onPreferenceError)
                }
            }) { Text("Save track preferences") }
        }
        title.seasons.forEach { season ->
            val isExpanded = expandedSeasons.contains(season.label)
            item(key = "season/${season.label}") {
                // A header, not a row with a button parked on the end of it: the
                // label and its count are the only things that grow, and the one
                // action is an icon of fixed width, so every season header down
                // the page starts and ends in the same place.
                Row(
                    Modifier
                        .fillMaxWidth()
                        .padding(top = 12.dp)
                        .clip(MaterialTheme.shapes.medium)
                        .clickable {
                            expandedSeasons = if (isExpanded) expandedSeasons - season.label else expandedSeasons + season.label
                        }
                        .padding(vertical = 8.dp, horizontal = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Icon(
                        if (isExpanded) Icons.Default.ExpandLess else Icons.Default.ExpandMore,
                        contentDescription = if (isExpanded) "Collapse" else "Expand",
                    )
                    Spacer(Modifier.width(10.dp))
                    Column(Modifier.weight(1f)) {
                        Text(
                            season.label,
                            style = MaterialTheme.typography.titleMedium,
                            fontWeight = FontWeight.SemiBold,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        val watched = season.episodes.count(EpisodeRecord::watched)
                        Text(
                            "${season.episodes.size} " +
                                (if (season.episodes.size == 1) "episode" else "episodes") +
                                if (watched > 0) " · $watched watched" else "",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    IconButton(onClick = { DownloadCoordinator.enqueue(context, season.episodes) }) {
                        Icon(
                            Icons.Default.Download,
                            contentDescription = "Download ${season.label}",
                        )
                    }
                }
            }
            if (isExpanded) {
                itemsIndexed(season.episodes, key = { _, it -> it.linkId }) { position, episode ->
                    EpisodeRow(
                        episode = episode,
                        numbering = episode.numbering(season.number, position),
                        download = downloads.firstOrNull {
                            it.shareId == episode.shareId && it.linkId == episode.linkId
                        },
                        playerReady = playerReady,
                        onPlay = { onPlay(playlist, playlist.indexOfFirst { it.linkId == episode.linkId }) },
                        onDownload = { DownloadCoordinator.enqueue(context, episode) },
                        onPause = { DownloadCoordinator.pause(context, it) },
                        onResume = { DownloadCoordinator.resume(context, it) },
                        onSetWatched = { onSetWatched(episode, it) },
                    )
                }
            }
        }
    }
    if (showMatch) {
        ChangeMatchDialog(
            title = title,
            onDismiss = { showMatch = false },
            onChanged = { showMatch = false; onMetadataChanged() },
            onError = onPreferenceError,
        )
    }
}

/**
 * `S03E38`, `E38`, or the row's place in the season.
 *
 * Mirrors `Episode::numbering` in `pstr-core`, plus the desktop client's
 * fallback to the season folder's number. The last case is deliberately *not*
 * printed as an episode number — nothing in the name numbered this file, and
 * `#4` says "fourth in the list" where `E04` would be a claim about the show.
 * Some ordinal is required regardless: a season of rows that all read the same
 * is a season the viewer cannot navigate at all.
 */
private fun EpisodeRecord.numbering(fallbackSeason: UInt?, position: Int): String {
    val season = this.season ?: fallbackSeason
    val number = this.number
    return when {
        season != null && number != null -> "S%02dE%02d".format(season.toInt(), number.toInt())
        number != null -> "E%02d".format(number.toInt())
        else -> "#${position + 1}"
    }
}

/**
 * One episode: play it, see where it was left, and drive its download.
 *
 * The row *is* the play button — the still, the numbering and the name are one
 * target, which is what lets the trailing controls be two icons in a fixed
 * column rather than a second row of buttons under every episode. Both were
 * needed while the row carried a full-width `Play`: it pushed the download and
 * watched controls into whatever width was left, and forty of those down a
 * season read as forty differently-arranged rows.
 */
@Composable
private fun EpisodeRow(
    episode: EpisodeRecord,
    numbering: String,
    download: RetainedDownload?,
    playerReady: Boolean,
    onPlay: () -> Unit,
    onDownload: () -> Unit,
    onPause: (RetainedDownload) -> Unit,
    onResume: (RetainedDownload) -> Unit,
    onSetWatched: (Boolean) -> Unit,
) {
    val running = download?.status == RetainedDownload.STATUS_RUNNING ||
        download?.status == RetainedDownload.STATUS_QUEUED
    // The provider's name for the episode when there is one: "The Cave of
    // Skulls" says more than the filename it was parsed out of. The filename
    // stays underneath it, and is the whole answer when there is no provider.
    val name = episode.providerName ?: episode.detail
    val detail = episode.detail.takeIf { it != name }
    Card(
        Modifier
            .fillMaxWidth()
            .clickable(enabled = playerReady, onClick = onPlay),
    ) {
        Column(Modifier.padding(10.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                // With metadata off the provider has no still, and Proton's own
                // thumbnail is a frame of this very episode.
                Box(
                    Modifier
                        .width(96.dp)
                        .aspectRatio(16f / 9f)
                        .clip(MaterialTheme.shapes.small),
                    contentAlignment = Alignment.Center,
                ) {
                    RemoteArtwork(
                        episode.stillUrl,
                        name,
                        Modifier.fillMaxSize(),
                        fallback = episode.thumbnailSource,
                    )
                    // Over the still rather than beside it: the row already
                    // plays on tap, so this says so without spending a column.
                    Icon(
                        Icons.Default.PlayArrow,
                        contentDescription = null,
                        tint = Color.White,
                        modifier = Modifier
                            .size(30.dp)
                            .clip(CircleShape)
                            .background(Color.Black.copy(alpha = 0.45f))
                            .padding(4.dp),
                    )
                }
                Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                    Text(
                        numbering,
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.primary,
                        fontWeight = FontWeight.SemiBold,
                        maxLines = 1,
                    )
                    Text(
                        name,
                        fontWeight = FontWeight.SemiBold,
                        // Seen episodes stay legible but stop competing with the
                        // one the viewer has not watched yet.
                        color = if (episode.watched) {
                            MaterialTheme.colorScheme.onSurfaceVariant
                        } else {
                            MaterialTheme.colorScheme.onSurface
                        },
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.padding(top = 2.dp),
                    )
                    detail?.let {
                        Text(
                            it,
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
                // Watched is a judgement the viewer is allowed to overrule: a
                // half-watched episode they are done with, or one the 90 % rule
                // marked seen because they sat through the credits.
                IconButton(onClick = { onSetWatched(!episode.watched) }) {
                    Icon(
                        if (episode.watched) Icons.Default.CheckCircle else Icons.Outlined.CheckCircle,
                        contentDescription = if (episode.watched) "Mark unwatched" else "Mark watched",
                        tint = if (episode.watched) {
                            MaterialTheme.colorScheme.primary
                        } else {
                            MaterialTheme.colorScheme.outline
                        },
                    )
                }
                // One slot, always the same width, whatever state the download
                // is in — including "there is nothing to say", where an empty
                // box keeps the rows above and below it aligned.
                Box(Modifier.size(48.dp), contentAlignment = Alignment.Center) {
                    when {
                        episode.offline -> Icon(
                            Icons.Default.CheckCircle,
                            contentDescription = "Saved offline",
                            tint = MaterialTheme.colorScheme.tertiary,
                        )
                        running -> IconButton(onClick = { onPause(download!!) }) {
                            Icon(Icons.Default.Pause, contentDescription = "Pause download")
                        }
                        download != null -> IconButton(onClick = { onResume(download) }) {
                            Icon(Icons.Default.Download, contentDescription = "Resume download")
                        }
                        else -> IconButton(onClick = onDownload) {
                            Icon(Icons.Default.Download, contentDescription = "Download")
                        }
                    }
                }
            }
            // Where the episode was left, under the row it belongs to.
            episode.progress?.takeIf { !episode.watched }?.let { progress ->
                AccentProgress(
                    progress = { progress.toFloat().coerceIn(0f, 1f) },
                    modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
                )
            }
            download?.takeIf { it.total > 0L && !episode.offline }?.let { active ->
                AccentProgress(
                    progress = { (active.downloaded.toFloat() / active.total).coerceIn(0f, 1f) },
                    modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
                    fill = solid(MaterialTheme.colorScheme.tertiary),
                )
                Text(
                    "${active.status.replaceFirstChar { it.uppercase() }} · " +
                        "${formatBytes(active.downloaded.toULong())} of ${formatBytes(active.total.toULong())}",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

@Composable
private fun ChangeMatchDialog(
    title: TitleRecord,
    onDismiss: () -> Unit,
    onChanged: () -> Unit,
    onError: (Throwable) -> Unit,
) {
    var term by remember(title.key) { mutableStateOf(title.canonicalName ?: title.name) }
    var searching by remember { mutableStateOf(false) }
    var options by remember { mutableStateOf<List<MatchRecord>>(emptyList()) }
    val scope = rememberCoroutineScope()
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Change match") },
        text = {
            Column {
                Text("Search the configured metadata provider. Nothing is stored until you choose an entry.")
                OutlinedTextField(term, { term = it }, Modifier.fillMaxWidth().padding(vertical = 8.dp), singleLine = true)
                AccentButton(enabled = term.isNotBlank() && !searching, onClick = {
                    searching = true
                    scope.launch {
                        runCatching {
                            withContext(Dispatchers.IO) { NativeRuntime.engine().searchMatches(title.key, term) }
                        }.onSuccess { options = it }.onFailure(onError)
                        searching = false
                    }
                }) { Text(if (searching) "Searching…" else "Search") }
                LazyColumn(Modifier.fillMaxWidth().heightIn(max = 300.dp)) {
                    items(options, key = { "${it.provider}:${it.remoteId}" }) { option ->
                        EdgedButton(
                            onClick = {
                                scope.launch {
                                    runCatching {
                                        withContext(Dispatchers.IO) { NativeRuntime.engine().chooseMatch(title.key, option) }
                                    }.onSuccess { onChanged() }.onFailure(onError)
                                }
                            },
                            modifier = Modifier.fillMaxWidth().padding(top = 6.dp),
                        ) {
                            Column(Modifier.fillMaxWidth()) {
                                Text(option.name, fontWeight = FontWeight.SemiBold)
                                Text(listOfNotNull(option.year?.toString(), option.originalName).joinToString(" · "))
                            }
                        }
                    }
                }
            }
        },
        confirmButton = {
            QuietButton(onClick = {
                scope.launch {
                    runCatching { withContext(Dispatchers.IO) { NativeRuntime.engine().forgetMatch(title.key) } }
                        .onSuccess { onChanged() }.onFailure(onError)
                }
            }) { Text("Forget match") }
        },
        dismissButton = { TonalButton(onClick = onDismiss) { Text("Close") } },
    )
}

@Composable
private fun SharesScreen(
    shares: List<ShareRecord>,
    onAdd: (String, String, String?) -> Unit,
    onRepair: (String, String, String?) -> Unit,
    onRefresh: (String) -> Unit,
    onRemove: (String) -> Unit,
    padding: PaddingValues,
) {
    var showAdd by remember { mutableStateOf(false) }
    var repairing by remember { mutableStateOf<ShareRecord?>(null) }
    Column(Modifier.fillMaxSize().padding(padding).padding(16.dp)) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
            Text("Proton Drive public links", style = MaterialTheme.typography.titleLarge)
            AccentButton(onClick = { showAdd = true }) { Text("Add share") }
        }
        LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 16.dp)) {
            items(shares, key = { it.id }) { share ->
                Card(Modifier.fillMaxWidth()) {
                    Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text(share.name, fontWeight = FontWeight.SemiBold)
                            Text(if (share.hasCustomPassword) "Custom password stored securely" else "Public link")
                        }
                        // One share, not the library: a link that has just had
                        // files added to it should not cost a walk of every
                        // other one, and a link that has expired should not stop
                        // the ones that still work from being refreshed.
                        IconButton(onClick = { onRefresh(share.id) }) {
                            Icon(Icons.Default.Refresh, contentDescription = "Refresh this share")
                        }
                        // Not a remove: a share whose secret has become
                        // unreadable cannot be removed either, since removal
                        // deletes a secret the store can no longer touch.
                        QuietButton(onClick = { repairing = share }) { Text("Re-enter link") }
                        TonalButton(onClick = { onRemove(share.id) }) { Text("Remove") }
                    }
                }
            }
        }
    }
    if (showAdd) AddShareDialog(onDismiss = { showAdd = false }, onAdd = onAdd)
    repairing?.let { share ->
        RepairShareDialog(
            share = share,
            onDismiss = { repairing = null },
            onRepair = { url, password -> onRepair(share.id, url, password) },
        )
    }
}

/**
 * Re-enter the link for a share the app can no longer decrypt its secret for.
 *
 * It has to be the *same* link — a different token is a different share, and
 * repointing this one at it would leave every catalog row and every offline file
 * describing something that is not there. Rust enforces that; this only says so.
 */
@Composable
private fun RepairShareDialog(
    share: ShareRecord,
    onDismiss: () -> Unit,
    onRepair: (String, String?) -> Unit,
) {
    var url by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(securePolicy = SecureFlagPolicy.SecureOn),
        title = { Text("Re-enter link for ${share.name}") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    "The stored credentials for this share cannot be read — usually after a " +
                        "screen-lock change or a device restore. Entering the same link again " +
                        "restores access without losing the library or anything downloaded.",
                    style = MaterialTheme.typography.bodySmall,
                )
                OutlinedTextField(
                    url,
                    { url = it },
                    label = { Text("Public share URL") },
                    singleLine = true,
                    visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                )
                OutlinedTextField(
                    password,
                    { password = it },
                    label = { Text("Custom password (optional)") },
                    singleLine = true,
                    visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                )
            }
        },
        confirmButton = {
            AccentButton(
                onClick = { onRepair(url.trim(), password); onDismiss() },
                enabled = url.isNotBlank(),
            ) { Text("Restore") }
        },
        dismissButton = { TonalButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
private fun AddShareDialog(onDismiss: () -> Unit, onAdd: (String, String, String?) -> Unit) {
    var name by remember { mutableStateOf("") }
    var url by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(securePolicy = SecureFlagPolicy.SecureOn),
        title = { Text("Add Proton Drive share") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(name, { name = it }, label = { Text("Library name") }, singleLine = true)
                OutlinedTextField(
                    url,
                    { url = it },
                    label = { Text("Public share URL") },
                    singleLine = true,
                    // The URL fragment is a secret. Password input disables
                    // IME learning/suggestions while normal long-press paste
                    // remains available.
                    visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                )
                OutlinedTextField(
                    password,
                    { password = it },
                    label = { Text("Custom password (optional)") },
                    singleLine = true,
                    visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                )
            }
        },
        confirmButton = {
            AccentButton(
                onClick = { onAdd(name.trim(), url.trim(), password); onDismiss() },
                enabled = name.isNotBlank() && url.isNotBlank(),
            ) { Text("Add") }
        },
        dismissButton = { TonalButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
private fun DownloadsScreen(
    state: AppUiState,
    onRemove: (uniffi.pstr_android.OfflineRecord) -> Unit,
    onPause: (RetainedDownload) -> Unit,
    onResume: (RetainedDownload) -> Unit,
    onDeletePartial: (RetainedDownload) -> Unit,
    padding: PaddingValues,
) {
    val context = LocalContext.current
    val work = WorkManager.getInstance(context).getWorkInfosByTagLiveData(DownloadCoordinator.TAG)
    val downloads by work.observeAsState(emptyList())
    // Reading retained metadata on every WorkInfo transition also hydrates
    // paused/failed/cancelled entries after WorkManager history is pruned.
    val retained = remember(downloads) { DownloadStateStore(context).records() }
    // Which show each saved file is from. The offline record knows its episode
    // but not its show, and the library is the only thing that does.
    val groups = remember(state.offline, state.titles) {
        val shows = state.titles.flatMap { title ->
            title.seasons.flatMap(SeasonRecord::episodes).map {
                "${it.shareId}/${it.linkId}" to (title.canonicalName ?: title.name)
            }
        }.toMap()
        state.offline
            .groupBy { shows["${it.shareId}/${it.linkId}"] ?: "Not in the library" }
            .toSortedMap()
    }
    LazyColumn(
        modifier = Modifier.fillMaxSize().padding(padding).padding(16.dp),
        contentPadding = PaddingValues(bottom = 16.dp),
    ) {
        item {
            Text("Offline downloads", style = MaterialTheme.typography.titleLarge)
            Spacer(Modifier.height(16.dp))
        }
        if (retained.isEmpty() && state.offline.isEmpty()) {
            item { EmptyState("No downloads", "Episodes, seasons, and shows saved offline appear here.") }
        }
        // Under the show they belong to, as on desktop: a saved season is
        // fourteen rows, and fourteen rows of "Episode 3" name nothing.
        groups.forEach { (show, files) ->
            item(key = "group/$show") {
                Text(show, style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 4.dp))
                Text(
                    "${files.size} ${if (files.size == 1) "episode" else "episodes"} · " +
                        formatBytes(files.sumOf { it.size }),
                    style = MaterialTheme.typography.bodySmall,
                    modifier = Modifier.padding(bottom = 8.dp),
                )
            }
            items(files, key = { "${it.shareId}/${it.linkId}" }) { file ->
                val episode = file.episode
                Card(Modifier.fillMaxWidth().padding(bottom = 8.dp)) {
                    Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text(episode?.label ?: file.linkId, fontWeight = FontWeight.SemiBold)
                            Text(formatBytes(file.size))
                        }
                        TonalButton(onClick = { onRemove(file) }) {
                            Icon(Icons.Default.Delete, contentDescription = null, modifier = Modifier.size(18.dp))
                            Spacer(Modifier.width(8.dp))
                            Text("Delete")
                        }
                    }
                }
            }
        }
        items(retained, key = { "${it.shareId}/${it.linkId}" }) { download ->
            val progress = if (download.total > 0L) download.downloaded.toFloat() / download.total else 0f
            Card(Modifier.fillMaxWidth().padding(bottom = 8.dp)) {
                Column(Modifier.padding(16.dp)) {
                    Text(download.label, fontWeight = FontWeight.SemiBold)
                    Text(download.status.replaceFirstChar { it.uppercase() })
                    if (download.total > 0L) {
                        AccentProgress(
                            progress = { progress },
                            modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
                        )
                        Text("${formatBytes(download.downloaded.toULong())} of ${formatBytes(download.total.toULong())}", style = MaterialTheme.typography.bodySmall)
                    }
                    download.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                    Row(
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                        modifier = Modifier.padding(top = 8.dp),
                    ) {
                        if (download.status == RetainedDownload.STATUS_RUNNING ||
                            download.status == RetainedDownload.STATUS_QUEUED
                        ) {
                            TonalButton(onClick = { onPause(download) }) { Text("Pause") }
                        } else {
                            TonalButton(onClick = { onResume(download) }) { Text("Resume") }
                        }
                        QuietButton(onClick = { onDeletePartial(download) }) { Text("Delete partial") }
                    }
                }
            }
        }
    }
}

@Composable
private fun SettingsScreen(
    state: AppUiState,
    onSaveMetadata: (Boolean, MetadataProvider, String, String) -> Unit,
    onMatchAgain: () -> Unit,
    onClearCache: () -> Unit,
    onRemoveAllOffline: () -> Unit,
    padding: PaddingValues,
) {
    val context = LocalContext.current
    val settings = remember { SettingsStore(context) }
    var wifiOnly by remember { mutableStateOf(settings.wifiOnly) }
    var backgroundAudio by remember { mutableStateOf(settings.backgroundAudio) }
    // Playback preferences are the shared store's, not this app's: they are the
    // same file the desktop client reads, so a language chosen on one is the
    // language the other starts in.
    var prefs by remember { mutableStateOf<PlaybackPrefsRecord?>(null) }
    val scope = rememberCoroutineScope()
    fun update(change: (PlaybackPrefsRecord) -> PlaybackPrefsRecord) {
        val next = change(prefs ?: return)
        prefs = next
        scope.launch(Dispatchers.IO) {
            runCatching { NativeRuntime.engine().setPlaybackPrefs(next) }
        }
    }
    LaunchedEffect(Unit) {
        runCatching { withContext(Dispatchers.IO) { NativeRuntime.engine().playbackPrefs() } }
            .onSuccess { prefs = it }
    }
    var confirmDelete by remember { mutableStateOf(false) }
    var showMetadata by remember { mutableStateOf(false) }
    var legalDocument by remember { mutableStateOf<LegalDocument?>(null) }
    Column(Modifier.fillMaxSize().padding(padding).padding(16.dp).verticalScroll(rememberScrollState())) {
        Text("Settings", style = MaterialTheme.typography.titleLarge)
        SettingToggle("Download on Wi-Fi only", wifiOnly) {
            wifiOnly = it
            settings.wifiOnly = it
            // Constraints are baked in at enqueue time, so a queue that already
            // exists keeps the policy it was queued under until it is re-issued.
            DownloadCoordinator.applyNetworkPolicy(context)
        }
        HorizontalDivider()
        SettingToggle("Continue audio in the background", backgroundAudio) {
            backgroundAudio = it
            settings.backgroundAudio = it
        }
        Text(
            "When disabled, leaving playback stops the player instead of keeping a media notification active.",
            style = MaterialTheme.typography.bodySmall,
            modifier = Modifier.padding(bottom = 14.dp),
        )
        HorizontalDivider()
        Text("Playback", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 20.dp))
        prefs?.let { current ->
            SettingToggle("Play the next episode automatically", current.autoplayNext) { on ->
                update { it.copy(autoplayNext = on) }
            }
            SettingToggle("Skip openings and endings automatically", current.autoSkip) { on ->
                update { it.copy(autoSkip = on) }
            }
            Text(
                "Openings and endings are read from the chapters a release was muxed with. " +
                    "With this off you get a Skip button instead, which is the safer default: " +
                    "a mis-named chapter then costs a tap rather than a scene.",
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.padding(bottom = 14.dp),
            )
            SettingToggle("Show subtitles", current.subtitles) { on ->
                update { it.copy(subtitles = on) }
            }
            // Language tags, not a picker: which languages exist is a property
            // of each file, and a list built from one episode is wrong for the
            // next. A show that has been given its own choice keeps it.
            LanguageField("Preferred audio language", current.audioLanguage) { tag ->
                update { it.copy(audioLanguage = tag) }
            }
            LanguageField("Preferred subtitle language", current.subtitleLanguage) { tag ->
                update { it.copy(subtitleLanguage = tag) }
            }
            Text(
                "Three-letter tags as they appear in the file — \"jpn\", \"eng\". Used for every " +
                    "title that has not been given a track choice of its own.",
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.padding(bottom = 14.dp),
            )
        }
        HorizontalDivider()
        Text("Appearance", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 20.dp))
        AppearancePicker()
        HorizontalDivider()
        Text("Metadata enrichment", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 20.dp))
        Text(
            if (state.metadataSettings.enabled) "On · ${state.metadataSettings.provider.displayName()}"
            else "Off (privacy default)",
        )
        Text(
            "Enabling this sends library title names to the selected third-party provider over HTTPS.",
            style = MaterialTheme.typography.bodySmall,
        )
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(vertical = 10.dp)) {
            TonalButton(onClick = { showMetadata = true }) { Text("Configure metadata") }
            // Every title looked up again, matched ones included: the way out of
            // a library the provider answered wrong, which otherwise stays wrong
            // for as long as the match is remembered.
            TonalButton(
                onClick = { onMatchAgain() },
                enabled = state.metadataSettings.enabled && !state.refreshing,
            ) { Text("Match everything again") }
        }
        HorizontalDivider()
        Text("Storage", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 20.dp))
        Text("Offline media is encrypted at rest by Android and kept in app-private storage.")
        Text(
            "${state.storage.offlineCount} episodes offline · ${formatBytes(state.storage.offlineBytes)}",
            modifier = Modifier.padding(top = 8.dp),
        )
        if (state.storage.partialBytes > 0uL) {
            Text(
                "Unfinished downloads · ${formatBytes(state.storage.partialBytes)}",
                style = MaterialTheme.typography.bodySmall,
            )
        }
        Text(
            "Streaming cache · ${formatBytes(state.storage.cacheBytes)}",
            style = MaterialTheme.typography.bodySmall,
        )
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 10.dp)) {
            // The cache is rebuildable, so it goes without asking. Offline
            // episodes are a choice the viewer made, so that one asks.
            TonalButton(onClick = onClearCache) { Text("Clear cache") }
            TonalButton(
                onClick = { confirmDelete = true },
                enabled = state.storage.offlineCount > 0uL,
            ) { Text("Delete all offline") }
        }
        Text("proton-stream Android · GPL-3.0-or-later", style = MaterialTheme.typography.bodySmall, modifier = Modifier.padding(top = 24.dp))
        Text(
            "This program comes with absolutely no warranty. You may redistribute it under the GNU GPL.",
            style = MaterialTheme.typography.bodySmall,
            modifier = Modifier.padding(top = 8.dp),
        )
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 12.dp)) {
            TonalButton(onClick = {
                legalDocument = LegalDocument("GNU GPL v3", "licenses/GPL-3.0.txt")
            }) { Text("View license") }
            TonalButton(onClick = {
                legalDocument = LegalDocument("Third-party notices", "licenses/THIRD_PARTY_NOTICES.md")
            }) { Text("View notices") }
        }
    }
    if (confirmDelete) {
        AlertDialog(
            onDismissRequest = { confirmDelete = false },
            title = { Text("Delete all offline episodes?") },
            text = {
                Text(
                    "${state.storage.offlineCount} episodes (${formatBytes(state.storage.offlineBytes)}) " +
                        "will be removed from this device. Watch history is kept, and anything " +
                        "deleted can be downloaded again.",
                )
            },
            confirmButton = {
                AccentButton(onClick = { confirmDelete = false; onRemoveAllOffline() }) { Text("Delete") }
            },
            dismissButton = { TonalButton(onClick = { confirmDelete = false }) { Text("Cancel") } },
        )
    }
    legalDocument?.let { document ->
        LegalDocumentDialog(document, onDismiss = { legalDocument = null })
    }
    if (showMetadata) {
        MetadataSettingsDialog(
            current = state.metadataSettings,
            onDismiss = { showMetadata = false },
            onSave = { enabled, provider, language, key ->
                onSaveMetadata(enabled, provider, language, key)
                showMetadata = false
            },
        )
    }
}

@Composable
private fun MetadataSettingsDialog(
    current: uniffi.pstr_android.MetadataSettingsRecord,
    onDismiss: () -> Unit,
    onSave: (Boolean, MetadataProvider, String, String) -> Unit,
) {
    var enabled by remember { mutableStateOf(current.enabled) }
    var provider by remember { mutableStateOf(current.provider) }
    var language by remember { mutableStateOf(current.language) }
    var apiKey by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(securePolicy = SecureFlagPolicy.SecureOn),
        title = { Text("Metadata enrichment") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Off by default: enabling sends the titles in your library to a third party, associated with your IP address and subject to their privacy policy.")
                SettingToggle("Enable enrichment", enabled) { enabled = it }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    MetadataProvider.entries.forEach { option ->
                        if (option == provider) AccentButton(onClick = { provider = option }) { Text(option.displayName()) }
                        else EdgedButton(onClick = { provider = option }) { Text(option.displayName()) }
                    }
                }
                Text(
                    if (provider == MetadataProvider.ANI_LIST) "Anime; no account or API key required."
                    else "Film and television; requires a free TMDB API key.",
                    style = MaterialTheme.typography.bodySmall,
                )
                if (provider == MetadataProvider.TMDB) {
                    OutlinedTextField(language, { language = it }, label = { Text("Language") }, singleLine = true)
                    OutlinedTextField(
                        apiKey,
                        { apiKey = it },
                        label = { Text(if (current.ready) "TMDB API key (leave blank to keep)" else "TMDB API key") },
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                        singleLine = true,
                    )
                }
            }
        },
        confirmButton = {
            AccentButton(
                enabled = !enabled || provider != MetadataProvider.TMDB || current.ready || apiKey.isNotBlank(),
                onClick = { onSave(enabled, provider, language.ifBlank { "en" }, apiKey) },
            ) { Text("Save") }
        },
        dismissButton = { TonalButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

private fun MetadataProvider.displayName(): String = when (this) {
    MetadataProvider.ANI_LIST -> "AniList"
    MetadataProvider.TMDB -> "TMDB"
}

private data class LegalDocument(val title: String, val assetPath: String)

@Composable
private fun LegalDocumentDialog(document: LegalDocument, onDismiss: () -> Unit) {
    val context = LocalContext.current
    var contents by remember(document.assetPath) { mutableStateOf("Loading…") }
    LaunchedEffect(document.assetPath) {
        contents = runCatching {
            withContext(Dispatchers.IO) {
                context.assets.open(document.assetPath).bufferedReader().use { it.readText() }
            }
        }.getOrElse { error -> "Unable to load this document: ${error.message}" }
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(document.title) },
        text = {
            LazyColumn(Modifier.fillMaxWidth().heightIn(max = 520.dp)) {
                item { Text(contents, style = MaterialTheme.typography.bodySmall) }
            }
        },
        confirmButton = { AccentButton(onClick = onDismiss) { Text("Close") } },
    )
}

@Composable
private fun SettingToggle(label: String, checked: Boolean, onChecked: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().padding(vertical = 14.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(label, Modifier.weight(1f))
        Switch(checked = checked, onCheckedChange = onChecked)
    }
}

/**
 * Flavour, accent and gradients — the same three the desktop client offers.
 *
 * Every colour is resolved by Rust, so a swatch here is the colour the app will
 * actually paint rather than an approximation of it, and the choice is stored in
 * the file both clients read.
 */
@Composable
private fun AppearancePicker() {
    val scope = rememberCoroutineScope()
    var choice by remember { mutableStateOf<AppearanceRecord?>(null) }
    var swatches by remember { mutableStateOf<Map<AccentChoice, Color>>(emptyMap()) }

    suspend fun repaint(next: AppearanceRecord, store: Boolean) {
        runCatching {
            withContext(Dispatchers.IO) {
                val engine = NativeRuntime.engine()
                if (store) engine.setAppearance(next)
                val palette = engine.previewPalette(next)
                // Every accent as it would look in *this* flavour: a swatch row
                // that keeps Mocha's pastels while Latte is selected is a row
                // that lies about what the next tap does.
                val row = AccentChoice.entries.associateWith { accent ->
                    Color(engine.previewPalette(next.copy(accent = accent)).accent.toInt())
                }
                palette to row
            }
        }.onSuccess { (palette, row) ->
            choice = next
            swatches = row
            AppearanceState.apply(palette)
        }
    }

    LaunchedEffect(Unit) {
        runCatching { withContext(Dispatchers.IO) { NativeRuntime.engine().appearance() } }
            .onSuccess { repaint(it, store = false) }
    }

    val current = choice ?: return
    Text("Palette", style = MaterialTheme.typography.bodyMedium, modifier = Modifier.padding(top = 8.dp))
    LazyRow(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(vertical = 8.dp)) {
        items(FlavorChoice.entries.toList(), key = { it.name }) { flavor ->
            val selected = flavor == current.flavor
            if (selected) {
                AccentButton(onClick = {}) { Text(flavor.label()) }
            } else {
                EdgedButton(onClick = {
                    scope.launch { repaint(current.copy(flavor = flavor), store = true) }
                }) { Text(flavor.label()) }
            }
        }
    }
    Text("Accent", style = MaterialTheme.typography.bodyMedium)
    LazyRow(horizontalArrangement = Arrangement.spacedBy(10.dp), modifier = Modifier.padding(vertical = 8.dp)) {
        items(AccentChoice.entries.toList(), key = { it.name }) { accent ->
            val swatch = swatches[accent] ?: MaterialTheme.colorScheme.surfaceVariant
            Box(
                Modifier
                    .size(36.dp)
                    .clip(CircleShape)
                    .background(swatch)
                    .border(
                        width = if (accent == current.accent) 3.dp else 1.dp,
                        color = if (accent == current.accent) {
                            MaterialTheme.colorScheme.onBackground
                        } else {
                            MaterialTheme.colorScheme.outline
                        },
                        shape = CircleShape,
                    )
                    .clickable {
                        scope.launch { repaint(current.copy(accent = accent), store = true) }
                    },
            )
        }
    }
    SettingToggle("Paint the accent as a gradient", current.gradients) { on ->
        scope.launch { repaint(current.copy(gradients = on), store = true) }
    }
    Text(
        "Off is the safer setting on a panel that bands: a slow ramp across a wide bar " +
            "shows every step it is drawn from, and flat is better than striped.",
        style = MaterialTheme.typography.bodySmall,
        modifier = Modifier.padding(bottom = 14.dp),
    )
}

/** What each palette family is called. The desktop client says the same. */
private fun FlavorChoice.label() = when (this) {
    FlavorChoice.PROTON -> "Proton"
    FlavorChoice.LATTE -> "Catppuccin Latte"
    FlavorChoice.FRAPPE -> "Catppuccin Frappé"
    FlavorChoice.MACCHIATO -> "Catppuccin Macchiato"
    FlavorChoice.MOCHA -> "Catppuccin Mocha"
}

/**
 * One language tag, committed as it is typed.
 *
 * Blank is a real answer and means "no preference" — the bridge stores it as
 * absent, which is what leaves the choice to the container's own default track.
 */
@Composable
private fun LanguageField(label: String, value: String?, onChange: (String?) -> Unit) {
    OutlinedTextField(
        value = value.orEmpty(),
        onValueChange = { onChange(it.trim().takeIf(String::isNotEmpty)) },
        label = { Text(label) },
        singleLine = true,
        modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
    )
}

@Composable
private fun EmptyState(title: String, body: String) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Text(title, style = MaterialTheme.typography.titleMedium)
            Text(body, style = MaterialTheme.typography.bodyMedium)
        }
    }
}

internal fun formatBytes(bytes: ULong): String {
    val value = bytes.toDouble()
    return when {
        value >= 1024 * 1024 * 1024 -> "%.1f GiB".format(value / (1024 * 1024 * 1024))
        value >= 1024 * 1024 -> "%.1f MiB".format(value / (1024 * 1024))
        value >= 1024 -> "%.1f KiB".format(value / 1024)
        else -> "$bytes bytes"
    }
}
