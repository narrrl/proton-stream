package io.narl.protonstream.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.Icon
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Search
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.narl.protonstream.ui.theme.AccentProgress
import uniffi.pstr_android.TitleRecord
import uniffi.pstr_android.EpisodeRecord
import uniffi.pstr_android.SeasonRecord

/**
 * How many shows the continue-watching shelf holds.
 *
 * A shelf is a shortcut, not a second library: past about this many it is
 * quicker to search for the thing than to scroll the shelf looking for it.
 */
private const val CONTINUE_WATCHING_MAX = 12

/** One row of the continue-watching shelf: which episode, and where it sits. */
private data class Resumable(val title: TitleRecord, val episode: EpisodeRecord, val index: Int)

@Composable
internal fun LibraryScreen(
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
