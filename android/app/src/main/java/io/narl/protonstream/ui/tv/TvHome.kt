package io.narl.protonstream.ui.tv

import io.narl.protonstream.ui.copperStar
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Info
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.RemoveCircleOutline
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.tv.material3.Button
import androidx.tv.material3.ExperimentalTvMaterial3Api
import androidx.tv.material3.FilterChip
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.Text
import io.narl.protonstream.ui.AppUiState

import io.narl.protonstream.ui.RemoteLogo
import io.narl.protonstream.ui.Resumable
import io.narl.protonstream.ui.caption
import io.narl.protonstream.ui.continueWatching
import io.narl.protonstream.ui.countdown
import io.narl.protonstream.ui.label
import io.narl.protonstream.ui.nextUpIndex
import io.narl.protonstream.ui.timeLeft
import io.narl.protonstream.ui.tryFocus
import kotlinx.coroutines.delay
import uniffi.pstr_android.EpisodeRecord
import uniffi.pstr_android.LibraryFilter
import uniffi.pstr_android.SeasonRecord
import uniffi.pstr_android.TitleRecord

/** What the home page can be asked to do. */
internal class HomeActions(
    val onPlay: (TitleRecord, Int) -> Unit,
    val onOpen: (TitleRecord) -> Unit,
    val onSetOnList: (TitleRecord, Boolean) -> Unit,
    val onSetTitleWatched: (TitleRecord, Boolean) -> Unit,
    val onForgetPosition: (TitleRecord, EpisodeRecord) -> Unit,
    val onFilter: (LibraryFilter) -> Unit,
    val onAddShare: () -> Unit,
)

/**
 * The television's library: what is in front at the top, over its backdrop,
 * and rows under it — Continue watching, the shelves (new episodes, My List,
 * what was added, and so on, as on the phone), then every title.
 *
 * The top follows the focus, so moving along a row shows each title's picture
 * and synopsis without opening it. It waits a moment first: a row run through
 * at the remote's repeat rate would otherwise load every backdrop on the way.
 */
@OptIn(ExperimentalTvMaterial3Api::class)
@Composable
internal fun TvHome(
    state: AppUiState,
    actions: HomeActions,
    rows: (String) -> LazyListState,
    page: LazyListState,
    heroKey: String?,
    onHero: (String) -> Unit,
) {
    val resumable = remember(state.titles) { continueWatching(state.titles) }
    val byKey = remember(state.titles) { state.titles.associateBy { it.key } }
    val shelves = remember(state.titles, state.arrangement) {
        state.arrangement?.shelves.orEmpty()
            .map { shelf -> shelf.label to shelf.keys.mapNotNull(byKey::get) }
            .filter { it.second.isNotEmpty() }
    }
    val tiles = remember(state.titles, state.arrangement) {
        state.arrangement?.tiles?.mapNotNull { tile -> byKey[tile.key]?.let { it to tile.folded.toInt() } }
            ?: state.titles.map { it to 0 }
    }
    var menuFor by remember { mutableStateOf<TitleRecord?>(null) }
    var menuResume by remember { mutableStateOf<Resumable?>(null) }

    when {
        state.loading -> return TvMessage("Loading the library…", null)
        state.titles.isEmpty() && state.activity != null -> return TvMessage(
            "Looking through the share",
            "${state.activity} Its titles appear here once it has been listed.",
        )
        state.titles.isEmpty() -> return TvMessage(
            "Your library is empty",
            "Sign in on Shares, and the shares you add on your phone arrive here. " +
                "Or add a share link there.",
            action = "Open Shares" to actions.onAddShare,
        )
    }

    val wanted = heroKey?.let(byKey::get) ?: resumable.firstOrNull()?.title ?: state.titles.first()
    var hero by remember { mutableStateOf(wanted) }
    LaunchedEffect(wanted.key) {
        if (wanted.key != hero.key) delay(HERO_SETTLE_MS)
        hero = wanted
    }
    val focus = { title: TitleRecord -> onHero(title.key) }

    Box(Modifier.fillMaxSize()) {
        PageBackdrop(hero)
        Column(Modifier.fillMaxSize()) {
            HeroText(hero, Modifier.fillMaxHeight(HERO_SHARE))
            ScrollsTo(ROW_LEAD) {
                LazyColumn(
                    state = page,
                    modifier = Modifier.fillMaxWidth().weight(1f),
                    contentPadding = PaddingValues(bottom = TV_TOP),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    if (resumable.isNotEmpty()) {
                        item(key = "continue") {
                            CardRow("Continue watching", state = rows("continue")) {
                                items(resumable, key = { "${it.episode.shareId}/${it.episode.linkId}" }) { entry ->
                                    EpisodeCard(
                                        entry.title,
                                        entry.episode,
                                        caption = if (entry.episode.resumeAt == null) {
                                            "Next: ${entry.episode.label}"
                                        } else {
                                            listOfNotNull(entry.episode.label, entry.episode.timeLeft()).joinToString(" · ")
                                        },
                                        onClick = { actions.onPlay(entry.title, entry.index) },
                                        onMenu = { menuResume = entry },
                                        onFocus = focus,
                                    )
                                }
                            }
                        }
                    }
                    // Only over the whole library: under a filter, a shelf of titles
                    // the rows below are hiding reads as the filter not working.
                    if (state.filter == LibraryFilter.ALL) {
                        shelves.forEach { (label, titles) ->
                            item(key = "shelf-$label") {
                                CardRow(label, state = rows("shelf-$label")) {
                                    items(titles, key = { it.key }) { title ->
                                        PosterCard(
                                            title,
                                            { actions.onOpen(title) },
                                            { menuFor = title },
                                            onFocus = focus,
                                            badge = title.badge ?: title.countdown(),
                                        )
                                    }
                                }
                            }
                        }
                    }
                    item(key = "all-heading") {
                        Column(Modifier.padding(horizontal = TV_SIDE).padding(top = 8.dp)) {
                            Text("All titles", style = MaterialTheme.typography.titleMedium)
                            Row(
                                Modifier.padding(top = 8.dp),
                                horizontalArrangement = Arrangement.spacedBy(8.dp),
                            ) {
                                LibraryFilter.entries.forEach { filter ->
                                    FilterChip(
                                        selected = filter == state.filter,
                                        onClick = { actions.onFilter(filter) },
                                    ) { Text(filter.label()) }
                                }
                            }
                        }
                    }
                    if (tiles.isEmpty()) {
                        item(key = "filtered-out") {
                            Text(
                                "Nothing in the library fits that filter.",
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                modifier = Modifier.padding(horizontal = TV_SIDE, vertical = 12.dp),
                            )
                        }
                    }
                    item(key = "all") {
                        PosterGrid(
                            tiles,
                            onOpen = actions.onOpen,
                            onMenu = { menuFor = it },
                            onFocus = focus,
                        )
                    }
                }
            }
        }
    }

    menuFor?.let { TitleMenu(it, actions) { menuFor = null } }
    menuResume?.let { entry ->
        TvMenu(
            entry.title.displayName,
            entry.episode.label,
            listOf(
                MenuEntry(Icons.Default.PlayArrow, "Resume") { actions.onPlay(entry.title, entry.index) },
                MenuEntry(Icons.Default.Info, "Open") { actions.onOpen(entry.title) },
                MenuEntry(Icons.Default.RemoveCircleOutline, "Remove from Continue watching") {
                    actions.onForgetPosition(entry.title, entry.episode)
                },
            ),
            onDismiss = { menuResume = null },
        )
    }
}

/**
 * Posters in rows as wide as the screen allows, each with how many more titles
 * of its franchise it stands for. Laid out as plain rows rather than a lazy
 * grid, which Compose does not allow inside a lazy page; a library is a few
 * hundred cards at most.
 */
@Composable
internal fun PosterGrid(
    tiles: List<Pair<TitleRecord, Int>>,
    onOpen: (TitleRecord) -> Unit,
    onMenu: (TitleRecord) -> Unit,
    onFocus: (TitleRecord) -> Unit = {},
) {
    BoxWithConstraints(Modifier.fillMaxWidth()) {
        val across = ((maxWidth - TV_SIDE * 2 + GRID_GAP) / (POSTER_WIDTH + GRID_GAP)).toInt().coerceAtLeast(1)
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            tiles.chunked(across).forEach { line ->
                Row(
                    Modifier.padding(horizontal = TV_SIDE, vertical = 12.dp),
                    horizontalArrangement = Arrangement.spacedBy(GRID_GAP),
                ) {
                    line.forEach { (title, folded) ->
                        PosterCard(
                            title,
                            { onOpen(title) },
                            { onMenu(title) },
                            onFocus = onFocus,
                            badge = if (folded > 0) "+$folded" else title.badge,
                        )
                    }
                }
            }
        }
    }
}

/** What Menu on a title's poster offers: play it, open it, list it, tick it off. */
@Composable
internal fun TitleMenu(title: TitleRecord, actions: HomeActions, onDismiss: () -> Unit) {
    val playlist = title.seasons.flatMap(SeasonRecord::episodes)
    val allWatched = title.episodeCount > 0uL && title.watchedCount == title.episodeCount
    TvMenu(
        title.displayName,
        title.caption(),
        listOf(
            MenuEntry(Icons.Default.PlayArrow, if (playlist.any { it.resumeAt != null }) "Resume" else "Play") {
                actions.onPlay(title, nextUpIndex(playlist, title.continueIndex))
            },
            MenuEntry(Icons.Default.Info, "Open") { actions.onOpen(title) },
            MenuEntry(
                if (title.onList) Icons.Default.Check else Icons.Default.Add,
                if (title.onList) "Remove from My List" else "Add to My List",
            ) { actions.onSetOnList(title, !title.onList) },
            MenuEntry(
                if (allWatched) Icons.Outlined.CheckCircle else Icons.Default.CheckCircle,
                if (allWatched) "Mark unwatched" else "Mark watched",
            ) { actions.onSetTitleWatched(title, !allWatched) },
        ),
        onDismiss = onDismiss,
    )
}

/** The title in front: its logo or name, what it is, and its synopsis. */
@Composable
private fun HeroText(title: TitleRecord, modifier: Modifier = Modifier) {
    Column(
        modifier.fillMaxWidth(HERO_TEXT_WIDTH).padding(start = TV_SIDE, top = TV_TOP, end = 16.dp, bottom = 8.dp),
        verticalArrangement = Arrangement.Bottom,
    ) {
        TitleName(title, Modifier.height(HERO_LOGO))
        Text(
            copperStar(heroLine(title)),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(top = 8.dp),
        )
        title.overview?.let {
            Text(
                it,
                style = MaterialTheme.typography.bodyMedium,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(top = 8.dp),
            )
        }
    }
}

/** A title's logo where the provider has one, its name in large type otherwise. */
@Composable
internal fun TitleName(title: TitleRecord, modifier: Modifier = Modifier) {
    Box(modifier, contentAlignment = Alignment.BottomStart) {
        RemoteLogo(title.logoUrl, title.displayName, Modifier.fillMaxHeight()) {
            Text(
                title.displayName,
                style = MaterialTheme.typography.displaySmall,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

/** "2023 · 12 episodes · ★ 8.4 · Drama, Mystery". */
internal fun heroLine(title: TitleRecord): String = listOfNotNull(
    title.caption(),
    title.rating?.let { "★ %.1f".format(it) },
    title.genres.take(3).joinToString(", ").takeIf { it.isNotEmpty() },
).joinToString(" · ")

/** A page with nothing to show yet: what is going on, and what to do about it. */
@Composable
internal fun TvMessage(heading: String, body: String?, action: Pair<String, () -> Unit>? = null) {
    Box(Modifier.fillMaxSize().padding(horizontal = TV_SIDE, vertical = TV_TOP), contentAlignment = Alignment.Center) {
        Column(
            Modifier.fillMaxWidth(0.6f),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(heading, style = MaterialTheme.typography.headlineMedium)
            body?.let {
                Text(it, style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            action?.let { (label, onClick) ->
                val first = remember { FocusRequester() }
                Button(onClick = onClick, modifier = Modifier.focusRequester(first)) { Text(label) }
                LaunchedEffect(Unit) { first.tryFocus() }
            }
        }
    }
}

/** How long the focus has to rest on a title before the top shows it. */
private const val HERO_SETTLE_MS = 300L

/** The share of the screen the title in front takes, above the rows. */
private const val HERO_SHARE = 0.44f

/** How far across the synopsis runs, leaving the picture clear to its right. */
private const val HERO_TEXT_WIDTH = 0.55f

private val HERO_LOGO = 60.dp
private val GRID_GAP = 20.dp
