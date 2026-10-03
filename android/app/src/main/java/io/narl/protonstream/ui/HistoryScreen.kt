package io.narl.protonstream.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.narl.protonstream.ui.theme.AccentProgress
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.TextStyle
import java.util.Locale
import uniffi.pstr_android.EpisodeRecord
import uniffi.pstr_android.SeasonRecord
import uniffi.pstr_android.TitleRecord

/** One played episode, with the title it belongs to and its place in that title's playlist. */
internal data class Played(val title: TitleRecord, val episode: EpisodeRecord, val index: Int)

/**
 * Every episode that has been started or finished, newest first — the
 * desktop's `Library::history`, over the records the library already holds.
 *
 * A row marked unwatched again sits at position zero, which is "never played"
 * for every purpose here, so it is left out. Progress stands in for the
 * position: it is non-zero exactly when the position is.
 */
internal fun history(titles: List<TitleRecord>): List<Played> =
    titles.flatMap { title ->
        title.seasons.flatMap(SeasonRecord::episodes).withIndex()
            .filter { (_, episode) -> episode.watched || (episode.progress ?: 0.0) > 0.0 }
            .map { (index, episode) -> Played(title, episode, index) }
    }.sortedByDescending { it.episode.lastPlayed }

/**
 * "Today", "Yesterday", a weekday within the last week, then a date — how the
 * history page heads each day.
 */
internal fun dayHeading(day: LocalDate, today: LocalDate, locale: Locale = Locale.getDefault()): String = when {
    day == today -> "Today"
    day == today.minusDays(1) -> "Yesterday"
    day.isAfter(today.minusDays(7)) -> day.dayOfWeek.getDisplayName(TextStyle.FULL, locale)
    day.year == today.year -> day.format(DateTimeFormatter.ofPattern("d MMMM", locale))
    else -> day.format(DateTimeFormatter.ofPattern("d MMMM yyyy", locale))
}

@Composable
internal fun HistoryScreen(
    state: AppUiState,
    onPlay: (TitleRecord, Int) -> Unit,
    onTitle: (TitleRecord) -> Unit,
    onRemove: (TitleRecord, EpisodeRecord) -> Unit,
    padding: PaddingValues,
    zone: ZoneId = remember { ZoneId.systemDefault() },
    today: LocalDate = LocalDate.now(zone),
) {
    val days = remember(state.titles) {
        history(state.titles).groupBy { Instant.ofEpochSecond(it.episode.lastPlayed).atZone(zone).toLocalDate() }
    }
    if (days.isEmpty()) {
        Box(Modifier.fillMaxSize().padding(padding)) {
            EmptyState("Nothing watched yet", "Episodes you play show up here, newest first.")
        }
        return
    }
    LazyColumn(Modifier.fillMaxSize().padding(padding), contentPadding = PaddingValues(bottom = 24.dp)) {
        days.forEach { (day, played) ->
            item(key = "day/$day") {
                Text(
                    dayHeading(day, today),
                    style = MaterialTheme.typography.titleSmall,
                    color = MaterialTheme.colorScheme.primary,
                    modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 20.dp, bottom = 4.dp),
                )
            }
            items(played, key = { "${it.episode.shareId}/${it.episode.linkId}" }) { entry ->
                HistoryRow(
                    entry,
                    onPlay = { onPlay(entry.title, entry.index) },
                    onTitle = { onTitle(entry.title) },
                    onRemove = { onRemove(entry.title, entry.episode) },
                )
            }
        }
    }
}

/**
 * The still plays the episode, the text opens its title, and the cross takes
 * it off the page — the same three targets the desktop row has.
 */
@Composable
private fun HistoryRow(entry: Played, onPlay: () -> Unit, onTitle: () -> Unit, onRemove: () -> Unit) {
    val episode = entry.episode
    Row(
        Modifier.fillMaxWidth().padding(start = 16.dp, end = 4.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            Modifier
                .width(128.dp)
                .aspectRatio(16f / 9f)
                .clip(MaterialTheme.shapes.small)
                .clickable(onClick = onPlay),
        ) {
            RemoteArtwork(
                episode.stillUrl ?: entry.title.backdropUrl,
                episode.label,
                Modifier.fillMaxSize(),
                fallback = episode.thumbnailSource,
            )
            episode.progress?.takeIf { !episode.watched }?.let { progress ->
                AccentProgress(
                    progress = { progress.toFloat().coerceIn(0f, 1f) },
                    modifier = Modifier.fillMaxWidth().align(Alignment.BottomCenter),
                )
            }
        }
        Column(Modifier.weight(1f).clickable(onClick = onTitle).padding(horizontal = 12.dp, vertical = 4.dp)) {
            Text(
                entry.title.canonicalName ?: entry.title.name,
                style = MaterialTheme.typography.titleSmall,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                episode.providerName?.let { "${episode.label} · $it" } ?: episode.label,
                style = MaterialTheme.typography.bodySmall,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                if (episode.watched) "Finished" else episode.stoppedAt(),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        IconButton(onClick = onRemove) {
            Icon(Icons.Default.Close, contentDescription = "Remove from history")
        }
    }
}

/** "Stopped at 12:04 of 23:40", or as much of it as is known. */
private fun EpisodeRecord.stoppedAt(): String {
    val at = resumeAt ?: return "Started"
    val duration = progress?.takeIf { it > 0.0 }?.let { at / it }
    return "Stopped at ${clock(at)}" + (duration?.let { " of ${clock(it)}" } ?: "")
}

private fun clock(seconds: Double): String {
    val total = seconds.toInt().coerceAtLeast(0)
    val hours = total / 3_600
    return if (hours > 0) "%d:%02d:%02d".format(hours, total % 3_600 / 60, total % 60)
    else "%d:%02d".format(total / 60, total % 60)
}
