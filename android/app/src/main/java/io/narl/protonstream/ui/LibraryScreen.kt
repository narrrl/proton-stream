package io.narl.protonstream.ui

import androidx.compose.foundation.background
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.LargeTopAppBar
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Row
import androidx.compose.ui.graphics.Brush
import io.narl.protonstream.ui.theme.AccentButton
import io.narl.protonstream.ui.theme.TonalButton
import java.time.LocalDate
import androidx.compose.foundation.combinedClickable
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Info
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.RemoveCircleOutline
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import io.narl.protonstream.download.DownloadCoordinator
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.narl.protonstream.ui.theme.AccentProgress
import androidx.compose.material.icons.automirrored.filled.Sort
import androidx.compose.material.icons.filled.Check
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import uniffi.pstr_android.EpisodeRecord
import uniffi.pstr_android.LibrarySort
import uniffi.pstr_android.SeasonRecord
import uniffi.pstr_android.TitleRecord
import uniffi.pstr_android.TitleType

/**
 * How many shows the continue-watching shelf holds.
 *
 * A shelf is a shortcut, not a second library: past about this many it is
 * quicker to search for the thing than to scroll the shelf looking for it.
 */
private const val CONTINUE_WATCHING_MAX = 12

/** One row of the continue-watching shelf: which episode, and where it sits. */
private data class Resumable(val title: TitleRecord, val episode: EpisodeRecord, val index: Int)

/** One tile of the grid: the title drawn, and how many more of its franchise it stands for. */
private data class Tile(val title: TitleRecord, val folded: Int)

/** A row above the grid: titles that share a director, a studio or a genre. */
private data class Shelf(val label: String, val titles: List<TitleRecord>)

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun LibraryScreen(
    state: AppUiState,
    onSearch: (String) -> Unit,
    onResume: (TitleRecord, Int) -> Unit,
    onTitle: (TitleRecord) -> Unit,
    onRefresh: () -> Unit,
    onSetTitleWatched: (TitleRecord, Boolean) -> Unit,
    onForgetPosition: (TitleRecord, EpisodeRecord) -> Unit,
    padding: PaddingValues,
    onMatchChanged: () -> Unit = {},
    onError: (Throwable) -> Unit = {},
    onArrange: (LibrarySort, Boolean) -> Unit = { _, _ -> },
) {
    val context = LocalContext.current
    // What a long press opened a menu for: a title, or a Continue watching card.
    var menuFor by remember { mutableStateOf<TitleRecord?>(null) }
    var menuResume by remember { mutableStateOf<Resumable?>(null) }
    var matching by remember { mutableStateOf<TitleRecord?>(null) }
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
    // Search is an icon until it is wanted: a permanent field above the
    // library spent a band of every visit on something used in few of them.
    var searching by rememberSaveable { mutableStateOf(state.query.isNotEmpty()) }
    val closeSearch = {
        searching = false
        onSearch("")
    }
    BackHandler(enabled = searching, onBack = closeSearch)
    // The large title folds into the bar as the grid scrolls, and comes back
    // only at the top: the page says what it is on arrival, then gets out of
    // the way.
    val scroll = TopAppBarDefaults.exitUntilCollapsedScrollBehavior()
    Column(Modifier.fillMaxSize().nestedScroll(scroll.nestedScrollConnection)) {
        if (searching) {
            SearchBar(state.query, onSearch, closeSearch)
        } else {
            LargeTopAppBar(
                title = { Text("Library") },
                actions = {
                    IconButton(onClick = { searching = true }) {
                        Icon(Icons.Default.Search, contentDescription = "Search library")
                    }
                    ArrangeMenu(state.sort, state.grouped, onArrange)
                    // Pull to refresh is the gesture; the icon stays for a
                    // viewer who does not know it.
                    IconButton(onClick = onRefresh, enabled = !state.refreshing) {
                        Icon(Icons.Default.Refresh, contentDescription = "Refresh")
                    }
                },
                colors = pageBarColors(),
                scrollBehavior = scroll,
            )
        }
        PullToRefreshBox(
            isRefreshing = state.refreshing,
            onRefresh = onRefresh,
            modifier = Modifier.weight(1f).padding(padding),
        ) {
            when {
                state.loading -> LibrarySkeleton()
                state.titles.isEmpty() && state.query.isNotBlank() -> EmptyState(
                    "Nothing matches",
                    "No title in the library is called “${state.query.trim()}”.",
                )
                state.titles.isEmpty() -> EmptyState(
                    "Your library is empty",
                    "Add a Proton Drive public link under Shares, then refresh.",
                )
                else -> LibraryGrid(
                    state,
                    resumable,
                    onResume,
                    onTitle,
                    onTitleMenu = { menuFor = it },
                    onResumeMenu = { menuResume = it },
                )
            }
        }
    }
    menuFor?.let { title ->
        val playlist = title.seasons.flatMap(SeasonRecord::episodes)
        val allWatched = title.episodeCount > 0uL && title.watchedCount == title.episodeCount
        TileMenu(title.displayName, title.caption(), onDismiss = { menuFor = null }) {
            MenuRow(Icons.Default.PlayArrow, if (playlist.any { it.resumeAt != null }) "Resume" else "Play") {
                onResume(title, nextUpIndex(playlist))
            }
            MenuRow(Icons.Default.Info, "Open") { onTitle(title) }
            MenuRow(Icons.Default.Download, "Download all") { DownloadCoordinator.enqueue(context, playlist) }
            MenuRow(
                if (allWatched) Icons.Outlined.CheckCircle else Icons.Default.CheckCircle,
                if (allWatched) "Mark unwatched" else "Mark watched",
            ) { onSetTitleWatched(title, !allWatched) }
            // The same search the title page offers, for the tile that is
            // plainly wrong — the wrong poster is what the grid shows.
            MenuRow(Icons.Default.Edit, "Change match") { matching = title }
        }
    }
    matching?.let { title ->
        ChangeMatchDialog(
            title = title,
            onDismiss = { matching = null },
            onChanged = {
                matching = null
                onMatchChanged()
            },
            onError = onError,
        )
    }
    menuResume?.let { entry ->
        TileMenu(
            entry.title.displayName,
            entry.episode.label,
            onDismiss = { menuResume = null },
        ) {
            MenuRow(Icons.Default.PlayArrow, "Resume") { onResume(entry.title, entry.index) }
            MenuRow(Icons.Default.Info, "Open") { onTitle(entry.title) }
            MenuRow(Icons.Default.RemoveCircleOutline, "Remove from Continue watching") {
                onForgetPosition(entry.title, entry.episode)
            }
        }
    }
}

/**
 * The grid's order, and whether a franchise is one tile or one per title —
 * the same choices the desktop offers beside its grid, behind one icon here
 * because a phone's bar has room for three.
 */
@Composable
private fun ArrangeMenu(sort: LibrarySort, grouped: Boolean, onArrange: (LibrarySort, Boolean) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { open = true }) {
            Icon(Icons.AutoMirrored.Filled.Sort, contentDescription = "Sort library")
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            LibrarySort.entries.forEach { option ->
                DropdownMenuItem(
                    text = { Text(option.label()) },
                    leadingIcon = {
                        if (option == sort) Icon(Icons.Default.Check, contentDescription = null)
                    },
                    onClick = {
                        open = false
                        onArrange(option, grouped)
                    },
                )
            }
            HorizontalDivider()
            DropdownMenuItem(
                text = { Text("One tile per franchise") },
                leadingIcon = {
                    if (grouped) Icon(Icons.Default.Check, contentDescription = null)
                },
                onClick = {
                    open = false
                    onArrange(sort, !grouped)
                },
            )
        }
    }
}

/** What each order is called. The desktop says the same. */
private fun LibrarySort.label() = when (this) {
    LibrarySort.NAME -> "A – Z"
    LibrarySort.RECENT -> "Recently watched"
    LibrarySort.ADDED -> "Recently added"
    LibrarySort.RELEASE -> "Newest"
    LibrarySort.RATING -> "Highest rated"
    LibrarySort.POPULARITY -> "Most popular"
}

/**
 * The bar while searching: back closes the search and clears it, and the field
 * takes the keyboard as soon as it opens, since opening it is asking to type.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SearchBar(query: String, onSearch: (String) -> Unit, onClose: () -> Unit) {
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }
    TopAppBar(
        navigationIcon = {
            IconButton(onClick = onClose) {
                Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Close search")
            }
        },
        title = {
            // A filled pill rather than an outlined form field: search is how
            // the page is navigated, not a value being entered.
            TextField(
                value = query,
                onValueChange = onSearch,
                modifier = Modifier.fillMaxWidth().padding(end = 8.dp).focusRequester(focus),
                singleLine = true,
                shape = CircleShape,
                placeholder = { Text("Search library") },
                trailingIcon = {
                    if (query.isNotEmpty()) {
                        IconButton(onClick = { onSearch("") }) {
                            Icon(Icons.Default.Close, contentDescription = "Clear search")
                        }
                    }
                },
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                colors = TextFieldDefaults.colors(
                    focusedIndicatorColor = Color.Transparent,
                    unfocusedIndicatorColor = Color.Transparent,
                    disabledIndicatorColor = Color.Transparent,
                ),
            )
        },
        colors = pageBarColors(),
    )
}

/**
 * A long press's menu, from the bottom of the screen where the thumb already
 * is. Each row closes it before doing its thing.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun TileMenu(
    heading: String,
    caption: String,
    onDismiss: () -> Unit,
    rows: @Composable MenuScope.() -> Unit,
) {
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.padding(bottom = 24.dp)) {
            Text(
                heading,
                style = MaterialTheme.typography.titleMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(horizontal = 24.dp),
            )
            Text(
                caption,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(start = 24.dp, end = 24.dp, bottom = 8.dp),
            )
            MenuScope(onDismiss).rows()
        }
    }
}

private class MenuScope(val dismiss: () -> Unit)

@Composable
private fun MenuScope.MenuRow(icon: ImageVector, label: String, onClick: () -> Unit) {
    ListItem(
        headlineContent = { Text(label) },
        leadingContent = { Icon(icon, contentDescription = null) },
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
        modifier = Modifier.clickable { dismiss(); onClick() }.padding(horizontal = 8.dp),
    )
}

@Composable
private fun LibraryGrid(
    state: AppUiState,
    resumable: List<Resumable>,
    onResume: (TitleRecord, Int) -> Unit,
    onTitle: (TitleRecord) -> Unit,
    onTitleMenu: (TitleRecord) -> Unit,
    onResumeMenu: (Resumable) -> Unit,
) {
    // The arrangement names titles by key; before it has arrived, the titles
    // as they came are the grid.
    val byKey = remember(state.titles) { state.titles.associateBy { it.key } }
    val tiles = remember(state.titles, state.arrangement) {
        state.arrangement?.tiles?.mapNotNull { tile -> byKey[tile.key]?.let { Tile(it, tile.folded.toInt()) } }
            ?: state.titles.map { Tile(it, 0) }
    }
    val shelves = remember(state.titles, state.arrangement) {
        state.arrangement?.shelves.orEmpty()
            .map { shelf -> Shelf(shelf.label, shelf.keys.mapNotNull(byKey::get)) }
            .filter { it.titles.isNotEmpty() }
    }
    // Posters, three across on a phone. Cropped backdrops at full width fitted
    // two titles to a screen, which is a library read one row at a time.
    LazyVerticalGrid(
        columns = GridCells.Adaptive(108.dp),
        modifier = Modifier.fillMaxSize(),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
        contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 8.dp, bottom = 24.dp),
    ) {
        // Above the grid, and only when there is something in it: what a viewer
        // opening the app wants is almost always the thing they were part way
        // through. Inside the grid rather than pinned over it, so the shelf
        // scrolls away with the rest.
        val featured = if (state.query.isBlank()) featured(state.titles, resumable) else null
        featured?.let { title ->
            item(key = "featured", span = { GridItemSpan(maxLineSpan) }) {
                FeaturedBanner(
                    title,
                    resume = resumable.firstOrNull { it.title.key == title.key },
                    onPlay = { onResume(title, it) },
                    onOpen = { onTitle(title) },
                )
            }
        }
        if (state.query.isBlank() && resumable.isNotEmpty()) {
            item(key = "continue-watching", span = { GridItemSpan(maxLineSpan) }) {
                Column {
                    Text("Continue watching", style = MaterialTheme.typography.titleMedium)
                    LazyRow(
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                        contentPadding = PaddingValues(top = 10.dp),
                    ) {
                        items(resumable, key = { "${it.episode.shareId}/${it.episode.linkId}" }) { entry ->
                            ContinueCard(
                                entry,
                                onClick = { onResume(entry.title, entry.index) },
                                onLongClick = { onResumeMenu(entry) },
                            )
                        }
                    }
                }
            }
        }
        if (state.query.isBlank()) {
            shelves.forEach { shelf ->
                item(key = "shelf-${shelf.label}", span = { GridItemSpan(maxLineSpan) }) {
                    Column {
                        Text(shelf.label, style = MaterialTheme.typography.titleMedium)
                        LazyRow(
                            horizontalArrangement = Arrangement.spacedBy(12.dp),
                            contentPadding = PaddingValues(top = 10.dp),
                        ) {
                            items(shelf.titles, key = { it.key }) { title ->
                                PosterTile(
                                    title,
                                    onClick = { onTitle(title) },
                                    onLongClick = { onTitleMenu(title) },
                                    modifier = Modifier.width(SHELF_TILE_WIDTH),
                                    shared = false,
                                )
                            }
                        }
                    }
                }
            }
            if (resumable.isNotEmpty() || shelves.isNotEmpty()) {
                item(key = "library-heading", span = { GridItemSpan(maxLineSpan) }) {
                    Text("All titles", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 8.dp))
                }
            }
        }
        items(tiles, key = { it.title.key }) { tile ->
            PosterTile(
                tile.title,
                onClick = { onTitle(tile.title) },
                onLongClick = { onTitleMenu(tile.title) },
                folded = tile.folded,
            )
        }
    }
}

/**
 * The grid's shape before the catalog has answered: posters and their two
 * lines of text as blocks, so the page does not jump when the titles land.
 */
@Composable
private fun LibrarySkeleton() {
    LazyVerticalGrid(
        columns = GridCells.Adaptive(108.dp),
        modifier = Modifier.fillMaxSize(),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
        contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 8.dp, bottom = 24.dp),
        userScrollEnabled = false,
    ) {
        items(SKELETON_TILES) {
            Column {
                Skeleton(Modifier.fillMaxWidth().aspectRatio(2f / 3f).clip(MaterialTheme.shapes.medium))
                Spacer(Modifier.height(8.dp))
                Skeleton(Modifier.fillMaxWidth(0.8f).height(12.dp).clip(MaterialTheme.shapes.small))
                Spacer(Modifier.height(6.dp))
                Skeleton(Modifier.fillMaxWidth(0.5f).height(10.dp).clip(MaterialTheme.shapes.small))
            }
        }
    }
}

/** A shelf's covers: a little narrower than the grid's, so the row reads as a row. */
private val SHELF_TILE_WIDTH = 112.dp

/** Enough placeholder tiles to fill a tablet's first screen. */
private const val SKELETON_TILES = 18

/**
 * The title the banner shows: the one watched last, or with nothing part
 * watched, one of the titles the provider has a backdrop for, turning over
 * once a day — the desktop's `featured`.
 */
private fun featured(titles: List<TitleRecord>, resumable: List<Resumable>): TitleRecord? {
    resumable.firstOrNull()?.let { return it.title }
    val pictured = titles.filter { it.backdropUrl != null }
    if (pictured.isEmpty()) return null
    return pictured[(LocalDate.now().toEpochDay() % pictured.size).toInt()]
}

/** Large art behind the name, rating, genres and three lines of synopsis. */
@Composable
private fun FeaturedBanner(title: TitleRecord, resume: Resumable?, onPlay: (Int) -> Unit, onOpen: () -> Unit) {
    val playlist = remember(title) { title.seasons.flatMap(SeasonRecord::episodes) }
    Box(Modifier.fillMaxWidth().aspectRatio(4f / 3f).clip(MaterialTheme.shapes.large)) {
        RemoteArtwork(
            title.backdropUrl ?: title.posterUrl,
            title.displayName,
            Modifier.fillMaxSize(),
            fallback = title.thumbnailSource,
            labelled = false,
        )
        Box(
            Modifier.fillMaxSize().background(
                Brush.verticalGradient(0.35f to Color.Transparent, 1f to Color.Black.copy(alpha = 0.85f)),
            ),
        )
        Column(Modifier.align(Alignment.BottomStart).padding(16.dp)) {
            Text(
                title.displayName,
                style = MaterialTheme.typography.headlineMedium,
                color = Color.White,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                listOfNotNull(
                    title.rating?.let { "★ %.1f".format(it) },
                    title.genres.takeIf { it.isNotEmpty() }?.take(3)?.joinToString(", "),
                ).joinToString("  ·  ").ifEmpty { title.caption() },
                style = MaterialTheme.typography.bodySmall,
                color = Color.White.copy(alpha = 0.8f),
            )
            title.overview?.let {
                Text(
                    it.trim(),
                    style = MaterialTheme.typography.bodySmall,
                    color = Color.White.copy(alpha = 0.8f),
                    maxLines = 3,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.padding(top = 6.dp),
                )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 12.dp)) {
                AccentButton(onClick = { onPlay(resume?.index ?: nextUpIndex(playlist)) }) {
                    Icon(Icons.Default.PlayArrow, contentDescription = null)
                    Spacer(Modifier.width(6.dp))
                    Text(if (resume != null) "Resume" else "Play")
                }
                TonalButton(onClick = onOpen) { Text("More info") }
            }
        }
    }
}

/** A part-watched episode: its still, where it was left, and what is left of it. */
@Composable
private fun ContinueCard(entry: Resumable, onClick: () -> Unit, onLongClick: () -> Unit) {
    Column(
        Modifier
            .width(220.dp)
            .clip(MaterialTheme.shapes.medium)
            .combinedClickable(onClick = onClick, onLongClick = onLongClick),
    ) {
        Box(Modifier.fillMaxWidth().aspectRatio(16f / 9f).clip(MaterialTheme.shapes.medium)) {
            RemoteArtwork(
                entry.episode.stillUrl ?: entry.title.backdropUrl,
                entry.title.displayName,
                Modifier.fillMaxSize(),
                fallback = entry.episode.thumbnailSource,
            )
            entry.episode.progress?.let { progress ->
                AccentProgress(
                    progress = { progress.toFloat().coerceIn(0f, 1f) },
                    modifier = Modifier.fillMaxWidth().align(Alignment.BottomCenter),
                )
            }
        }
        Text(
            entry.title.displayName,
            fontWeight = FontWeight.SemiBold,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(top = 8.dp),
        )
        Text(
            listOfNotNull(entry.episode.label, entry.episode.timeLeft()).joinToString(" · "),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/**
 * One title: its poster, and its name and size under it rather than in a card.
 *
 * [folded] is how many more titles of its franchise the tile stands for, said
 * in the corner. [shared] is whether the poster grows into the title page —
 * only one copy of a title on screen can, and a shelf may hold the same title
 * as the grid.
 */
@Composable
private fun PosterTile(
    title: TitleRecord,
    onClick: () -> Unit,
    onLongClick: () -> Unit,
    modifier: Modifier = Modifier,
    folded: Int = 0,
    shared: Boolean = true,
) {
    Column(
        modifier
            .clip(MaterialTheme.shapes.medium)
            .combinedClickable(onClick = onClick, onLongClick = onLongClick),
    ) {
        Box {
            RemoteArtwork(
                title.posterUrl ?: title.backdropUrl,
                title.displayName,
                (if (shared) Modifier.sharedArt(title.key) else Modifier)
                    .fillMaxWidth()
                    .aspectRatio(2f / 3f)
                    .clip(MaterialTheme.shapes.medium),
                fallback = title.thumbnailSource,
            )
            if (folded > 0) {
                Text(
                    "+$folded",
                    style = MaterialTheme.typography.labelSmall,
                    color = Color.White,
                    modifier = Modifier
                        .align(Alignment.TopEnd)
                        .padding(6.dp)
                        .clip(MaterialTheme.shapes.small)
                        .background(Color.Black.copy(alpha = 0.7f))
                        .padding(horizontal = 6.dp, vertical = 2.dp),
                )
            }
        }
        Spacer(Modifier.height(6.dp))
        Text(
            title.displayName,
            style = MaterialTheme.typography.bodyMedium,
            fontWeight = FontWeight.SemiBold,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
        )
        // Two lines, not one: at a large font scale "2023 · 7 episodes" does
        // not fit a poster's width, and one line cut it to "2023 · 7".
        Text(
            title.caption(),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** "2023 · 11 episodes", or "1988 · Film" — never "1 episodes". */
private fun TitleRecord.caption(): String = listOfNotNull(
    (metadataYear ?: year)?.toString(),
    when {
        (metadataKind ?: kind) == TitleType.FILM && episodeCount == 1uL -> "Film"
        episodeCount == 1uL -> "1 episode"
        else -> "$episodeCount episodes"
    },
).joinToString(" · ")

/**
 * "12 min left", from where the episode was left and how far through that is.
 * Null when either is unknown — a guess would be worse than no figure.
 */
private fun EpisodeRecord.timeLeft(): String? {
    val at = resumeAt ?: return null
    val fraction = progress?.takeIf { it > 0.0 && it < 1.0 } ?: return null
    val minutes = ((at / fraction - at) / 60).toInt()
    return if (minutes < 1) "under a minute left" else "$minutes min left"
}
