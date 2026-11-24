package io.narl.protonstream.ui.tv

import io.narl.protonstream.ui.copperStar
import io.narl.protonstream.ui.theme.CORNER
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.DoneAll
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Replay
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.tv.material3.Button
import androidx.tv.material3.ClickableSurfaceDefaults
import androidx.tv.material3.ExperimentalTvMaterial3Api
import androidx.tv.material3.FilterChip
import androidx.tv.material3.Icon
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.OutlinedButton
import androidx.tv.material3.OutlinedButtonDefaults
import androidx.tv.material3.Surface
import androidx.tv.material3.Text
import io.narl.protonstream.ui.RemoteArtwork
import io.narl.protonstream.ui.formatRuntime
import io.narl.protonstream.ui.heading
import io.narl.protonstream.ui.nextUpIndex
import io.narl.protonstream.ui.onMenuKey
import io.narl.protonstream.ui.statusText
import io.narl.protonstream.ui.theme.AccentProgress
import io.narl.protonstream.ui.thumbnailSource
import io.narl.protonstream.ui.timeLeft
import io.narl.protonstream.ui.tryFocus
import uniffi.pstr_android.EpisodeRecord
import uniffi.pstr_android.SeasonRecord
import uniffi.pstr_android.TitleRecord

/** What the title page can be asked to do. */
internal class TitleActions(
    /** Play this title's [Int]th episode, counted across its seasons. */
    val onPlay: (Int) -> Unit,
    val onSetOnList: (TitleRecord, Boolean) -> Unit,
    val onSetTitleWatched: (TitleRecord, Boolean) -> Unit,
    /** Several episodes seen or unseen at once, named for the message. */
    val onMarkEpisodes: (List<EpisodeRecord>, Boolean, String) -> Unit,
    val onOpenTitle: (TitleRecord) -> Unit,
)

/**
 * One title on a television: its backdrop behind it, what it is and the button
 * that plays it, then its seasons and episodes, then what else is like it.
 *
 * Opens with Play focused, so OK straight away does what most visits are for.
 * Moving down scrolls the page to the episodes; a season is picked from the
 * chips above them.
 */
@OptIn(ExperimentalTvMaterial3Api::class)
@Composable
internal fun TvTitle(
    title: TitleRecord,
    playerReady: Boolean,
    actions: TitleActions,
    franchise: List<TitleRecord> = emptyList(),
    moreLike: List<TitleRecord> = emptyList(),
    page: LazyListState = rememberLazyListState(),
) {
    val playlist = remember(title) { title.seasons.flatMap(SeasonRecord::episodes) }
    val nextUp = remember(playlist) { nextUpIndex(playlist, title.continueIndex) }
    val upNext = playlist.getOrNull(nextUp)
    var seasonIndex by remember(title.key) {
        mutableIntStateOf(
            title.seasons.indexOfFirst { season -> season.episodes.any { it.linkId == upNext?.linkId } }
                .coerceAtLeast(0),
        )
    }
    val season = title.seasons.getOrNull(seasonIndex)
    var menuFor by remember { mutableStateOf<EpisodeRecord?>(null) }
    var titleMenu by remember { mutableStateOf<TitleRecord?>(null) }
    val play = remember { FocusRequester() }
    LaunchedEffect(title.key) { play.tryFocus() }

    Box(Modifier.fillMaxSize()) {
        PageBackdrop(title, height = 0.85f)
        ScrollsJustEnough {
            LazyColumn(
                state = page,
                modifier = Modifier.fillMaxSize(),
                contentPadding = PaddingValues(top = TV_TOP + 24.dp, bottom = TV_TOP),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                item(key = "header") {
                    Column(Modifier.padding(start = TV_SIDE)) {
                        TitleName(title, Modifier.fillMaxWidth(HEADER_WIDTH).height(TITLE_LOGO))
                        Text(
                            copperStar(heroLine(title)),
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(top = 8.dp),
                        )
                        statusText(title)?.let { (text, _) ->
                            Text(
                                text,
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.primary,
                                modifier = Modifier.padding(top = 4.dp),
                            )
                        }
                        // The picked season's own name and synopsis, where the
                        // provider files it as an entry of its own.
                        season?.entryName?.let { name ->
                            Text(
                                listOfNotNull(name, season.year?.toString()).joinToString("  ·  "),
                                style = MaterialTheme.typography.titleSmall,
                                modifier = Modifier.padding(top = 12.dp),
                            )
                        }
                        (season?.overview ?: title.overview)?.let {
                            Text(
                                it,
                                style = MaterialTheme.typography.bodyMedium,
                                maxLines = 4,
                                overflow = TextOverflow.Ellipsis,
                                modifier = Modifier.fillMaxWidth(HEADER_WIDTH).padding(top = 12.dp),
                            )
                        }
                        // The secondary buttons on a wash of the page rather than
                        // bare: the row runs out over the artwork, and an outline
                        // alone left "Watched" white on a white sky.
                        val quiet = OutlinedButtonDefaults.colors(
                            containerColor = MaterialTheme.colorScheme.surface.copy(alpha = QUIET_BUTTON_ALPHA),
                        )
                        Row(
                            Modifier.padding(top = 20.dp),
                            horizontalArrangement = Arrangement.spacedBy(12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Button(
                                onClick = { actions.onPlay(nextUp) },
                                enabled = playerReady && playlist.isNotEmpty(),
                                modifier = Modifier.focusRequester(play),
                            ) {
                                Icon(Icons.Default.PlayArrow, contentDescription = null)
                                Spacer(Modifier.width(8.dp))
                                Text(
                                    when {
                                        !playerReady -> "Player loading…"
                                        upNext == null -> "Play"
                                        upNext.resumeAt != null -> "Resume ${upNext.label}"
                                        // Watched through, it starts the show again: nothing to continue.
                                        title.watchedCount > 0uL && !upNext.watched -> "Continue with ${upNext.label}"
                                        playlist.size == 1 -> "Play"
                                        else -> "Play ${upNext.label}"
                                    },
                                    maxLines = 1,
                                )
                            }
                            // Only where it would do something different: an unstarted
                            // show already starts at the beginning.
                            if (upNext?.resumeAt != null || title.watchedCount > 0uL) {
                                OutlinedButton(onClick = { actions.onPlay(0) }, enabled = playerReady, colors = quiet) {
                                    Icon(Icons.Default.Replay, contentDescription = null)
                                    Spacer(Modifier.width(8.dp))
                                    Text("Start over", maxLines = 1)
                                }
                            }
                            OutlinedButton(onClick = { actions.onSetOnList(title, !title.onList) }, colors = quiet) {
                                Icon(if (title.onList) Icons.Default.Check else Icons.Default.Add, contentDescription = null)
                                Spacer(Modifier.width(8.dp))
                                Text("My List", maxLines = 1)
                            }
                            val allWatched = title.episodeCount > 0uL && title.watchedCount == title.episodeCount
                            OutlinedButton(onClick = { actions.onSetTitleWatched(title, !allWatched) }, colors = quiet) {
                                Icon(
                                    if (allWatched) Icons.Default.CheckCircle else Icons.Outlined.CheckCircle,
                                    contentDescription = null,
                                )
                                Spacer(Modifier.width(8.dp))
                                Text(if (allWatched) "Watched" else "Mark watched", maxLines = 1)
                            }
                        }
                    }
                }
                if (title.seasons.size > 1) {
                    item(key = "seasons") {
                        Row(
                            Modifier.padding(start = TV_SIDE, top = 24.dp),
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            title.seasons.forEachIndexed { index, it ->
                                FilterChip(selected = index == seasonIndex, onClick = { seasonIndex = index }) {
                                    Text(it.label)
                                }
                            }
                        }
                    }
                }
                season?.let { shown ->
                    // Counted across seasons, which is how the player and the menu's
                    // "up to here" count.
                    val offset = title.seasons.take(seasonIndex).sumOf { it.episodes.size }
                    itemsIndexed(shown.episodes, key = { _, it -> "${it.shareId}/${it.linkId}" }) { index, episode ->
                        EpisodeLine(
                            episode,
                            episode.heading(index),
                            enabled = playerReady,
                            onPlay = { actions.onPlay(offset + index) },
                            onMenu = { menuFor = episode },
                            modifier = Modifier.padding(top = if (index == 0) 16.dp else 0.dp),
                        )
                    }
                }
                if (franchise.size > 1) {
                    item(key = "franchise") {
                        CardRow("In this franchise", Modifier.padding(top = 24.dp)) {
                            items(franchise, key = { it.key }) { other ->
                                PosterCard(
                                    other,
                                    { if (other.key != title.key) actions.onOpenTitle(other) },
                                    { titleMenu = other },
                                )
                            }
                        }
                    }
                }
                if (moreLike.isNotEmpty()) {
                    item(key = "more-like") {
                        CardRow("More like this", Modifier.padding(top = 24.dp)) {
                            items(moreLike, key = { it.key }) { other ->
                                PosterCard(other, { actions.onOpenTitle(other) }, { titleMenu = other })
                            }
                        }
                    }
                }
            }
        }
    }

    menuFor?.let { episode ->
        val at = playlist.indexOfFirst { it.linkId == episode.linkId }
        TvMenu(
            episode.label,
            title.displayName,
            listOfNotNull(
                MenuEntry(Icons.Default.PlayArrow, if (episode.resumeAt != null) "Resume" else "Play") {
                    actions.onPlay(at)
                }.takeIf { playerReady && at >= 0 },
                MenuEntry(
                    if (episode.watched) Icons.Outlined.CheckCircle else Icons.Default.CheckCircle,
                    if (episode.watched) "Mark unwatched" else "Mark watched",
                ) { actions.onMarkEpisodes(listOf(episode), !episode.watched, episode.label) },
                MenuEntry(Icons.Default.DoneAll, "Mark watched up to here") {
                    actions.onMarkEpisodes(playlist.take(at + 1), true, "${title.displayName} up to ${episode.label}")
                }.takeIf { at > 0 },
            ),
            onDismiss = { menuFor = null },
        )
    }
    titleMenu?.let { other ->
        TvMenu(
            other.displayName,
            null,
            listOf(
                MenuEntry(
                    if (other.onList) Icons.Default.Check else Icons.Default.Add,
                    if (other.onList) "Remove from My List" else "Add to My List",
                ) { actions.onSetOnList(other, !other.onList) },
            ),
            onDismiss = { titleMenu = null },
        )
    }
}

/**
 * One episode: a frame of it with how far through it is, its name, and the
 * provider's synopsis. OK plays it; Menu has the rest.
 */
@Composable
private fun EpisodeLine(
    episode: EpisodeRecord,
    heading: String,
    enabled: Boolean,
    onPlay: () -> Unit,
    onMenu: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Surface(
        onClick = onPlay,
        onLongClick = onMenu,
        enabled = enabled,
        modifier = modifier
            .padding(horizontal = TV_SIDE - EPISODE_INSET)
            .fillMaxWidth(EPISODE_WIDTH)
            .onMenuKey(onMenu),
        shape = ClickableSurfaceDefaults.shape(RoundedCornerShape(CORNER.dp)),
        colors = ClickableSurfaceDefaults.colors(
            containerColor = androidx.compose.ui.graphics.Color.Transparent,
            focusedContainerColor = MaterialTheme.colorScheme.surfaceVariant,
            // tv-material's focused text is for its own light focus colour; on
            // this one it would be dark on dark.
            contentColor = MaterialTheme.colorScheme.onSurface,
            focusedContentColor = MaterialTheme.colorScheme.onSurface,
        ),
        scale = ClickableSurfaceDefaults.scale(focusedScale = 1.02f),
        border = ClickableSurfaceDefaults.border(focusedBorder = focusBorder(RoundedCornerShape(CORNER.dp))),
    ) {
        Row(Modifier.padding(EPISODE_INSET), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.width(STILL_WIDTH).aspectRatio(16f / 9f)) {
                RemoteArtwork(
                    episode.stillUrl,
                    heading,
                    Modifier.fillMaxSize().clearAndSetSemantics {},
                    fallback = episode.thumbnailSource,
                    labelled = false,
                    maxEdge = STILL_WIDTH,
                )
                episode.progress?.takeIf { !episode.watched }?.let { progress ->
                    AccentProgress(
                        progress = { progress.toFloat() },
                        modifier = Modifier.align(Alignment.BottomCenter).padding(6.dp),
                    )
                }
                if (episode.watched) {
                    Icon(
                        Icons.Default.CheckCircle,
                        contentDescription = "Watched",
                        tint = MaterialTheme.colorScheme.primary,
                        modifier = Modifier.align(Alignment.TopEnd).padding(6.dp).size(20.dp),
                    )
                }
            }
            Column(Modifier.padding(start = 16.dp).weight(1f)) {
                Text(heading, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(
                    listOfNotNull(
                        episode.label,
                        episode.runtime?.let(::formatRuntime),
                        episode.timeLeft(),
                    ).joinToString(" · "),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                )
                episode.providerOverview?.let {
                    Text(
                        it,
                        style = MaterialTheme.typography.bodySmall,
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.padding(top = 4.dp),
                    )
                }
            }
        }
    }
}

/** How far across the header runs, leaving the backdrop clear to its right. */
private const val HEADER_WIDTH = 0.62f

/** How far across an episode runs: a line of synopsis, not a paragraph. */
private const val EPISODE_WIDTH = 0.8f

private val TITLE_LOGO = 88.dp
private val STILL_WIDTH = 176.dp
private val EPISODE_INSET = 8.dp

/** How much of the page shows under a secondary button over artwork. */
private const val QUIET_BUTTON_ALPHA = 0.72f
