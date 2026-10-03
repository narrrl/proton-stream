package io.narl.protonstream.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Row
import androidx.compose.ui.graphics.Brush
import io.narl.protonstream.ui.theme.AccentButton
import io.narl.protonstream.ui.theme.TonalButton
import java.time.LocalDate
import androidx.compose.foundation.combinedClickable
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Download
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
import androidx.compose.material3.CircularProgressIndicator
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
import uniffi.pstr_android.EpisodeRecord
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
) {
    val context = LocalContext.current
    // What a long press opened a menu for: a title, or a Continue watching card.
    var menuFor by remember { mutableStateOf<TitleRecord?>(null) }
    var menuResume by remember { mutableStateOf<Resumable?>(null) }
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
    Column(Modifier.fillMaxSize().padding(padding)) {
        // A filled pill rather than an outlined form field: search is how the
        // page is navigated, not a value being entered.
        TextField(
            value = state.query,
            onValueChange = onSearch,
            modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
            singleLine = true,
            shape = CircleShape,
            placeholder = { Text("Search library") },
            leadingIcon = { Icon(Icons.Default.Search, contentDescription = null) },
            trailingIcon = {
                if (state.query.isNotEmpty()) {
                    IconButton(onClick = { onSearch("") }) {
                        Icon(Icons.Default.Close, contentDescription = "Clear search")
                    }
                }
            },
            colors = TextFieldDefaults.colors(
                focusedIndicatorColor = Color.Transparent,
                unfocusedIndicatorColor = Color.Transparent,
                disabledIndicatorColor = Color.Transparent,
            ),
        )
        // Pull to refresh, the gesture every list on a phone answers to; the
        // icon in the bar stays for a viewer who does not know it.
        PullToRefreshBox(
            isRefreshing = state.refreshing,
            onRefresh = onRefresh,
            modifier = Modifier.fillMaxSize(),
        ) {
            when {
                state.loading -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator()
                }
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
        TileMenu(title.canonicalName ?: title.name, title.caption(), onDismiss = { menuFor = null }) {
            MenuRow(Icons.Default.PlayArrow, if (playlist.any { it.resumeAt != null }) "Resume" else "Play") {
                onResume(title, nextUpIndex(playlist))
            }
            MenuRow(Icons.Default.Info, "Open") { onTitle(title) }
            MenuRow(Icons.Default.Download, "Download all") { DownloadCoordinator.enqueue(context, playlist) }
            MenuRow(
                if (allWatched) Icons.Outlined.CheckCircle else Icons.Default.CheckCircle,
                if (allWatched) "Mark unwatched" else "Mark watched",
            ) { onSetTitleWatched(title, !allWatched) }
        }
    }
    menuResume?.let { entry ->
        TileMenu(
            entry.title.canonicalName ?: entry.title.name,
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
            item(key = "library-heading", span = { GridItemSpan(maxLineSpan) }) {
                Text("Library", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 8.dp))
            }
        }
        items(state.titles, key = { it.key }) { title ->
            PosterTile(title, onClick = { onTitle(title) }, onLongClick = { onTitleMenu(title) })
        }
    }
}

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
            title.canonicalName ?: title.name,
            Modifier.fillMaxSize(),
            fallback = title.thumbnailSource,
        )
        Box(
            Modifier.fillMaxSize().background(
                Brush.verticalGradient(0.35f to Color.Transparent, 1f to Color.Black.copy(alpha = 0.85f)),
            ),
        )
        Column(Modifier.align(Alignment.BottomStart).padding(16.dp)) {
            Text(
                title.canonicalName ?: title.name,
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
                entry.title.canonicalName ?: entry.title.name,
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
            entry.title.canonicalName ?: entry.title.name,
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

/** One title: its poster, and its name and size under it rather than in a card. */
@Composable
private fun PosterTile(title: TitleRecord, onClick: () -> Unit, onLongClick: () -> Unit) {
    Column(
        Modifier
            .clip(MaterialTheme.shapes.medium)
            .combinedClickable(onClick = onClick, onLongClick = onLongClick),
    ) {
        RemoteArtwork(
            title.posterUrl ?: title.backdropUrl,
            title.canonicalName ?: title.name,
            Modifier.fillMaxWidth().aspectRatio(2f / 3f).clip(MaterialTheme.shapes.medium),
            fallback = title.thumbnailSource,
        )
        Spacer(Modifier.height(6.dp))
        Text(
            title.canonicalName ?: title.name,
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
