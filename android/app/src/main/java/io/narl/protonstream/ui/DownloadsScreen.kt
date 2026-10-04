package io.narl.protonstream.ui

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
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.livedata.observeAsState
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.work.WorkManager
import io.narl.protonstream.download.DownloadCoordinator
import io.narl.protonstream.download.DownloadStateStore
import io.narl.protonstream.download.RetainedDownload
import io.narl.protonstream.ui.theme.AccentButton
import io.narl.protonstream.ui.theme.AccentProgress
import io.narl.protonstream.ui.theme.TonalButton
import uniffi.pstr_android.OfflineRecord
import uniffi.pstr_android.SeasonRecord
import uniffi.pstr_android.TitleRecord

/** One show's saved episodes, and the show itself where the library has it. */
private data class OfflineGroup(val name: String, val title: TitleRecord?, val files: List<OfflineRecord>)

@Composable
internal fun DownloadsScreen(
    state: AppUiState,
    onRemove: (OfflineRecord) -> Unit,
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
            title.seasons.flatMap(SeasonRecord::episodes).map { "${it.shareId}/${it.linkId}" to title }
        }.toMap()
        state.offline
            .groupBy { shows["${it.shareId}/${it.linkId}"] }
            .map { (title, files) ->
                OfflineGroup(title?.displayName ?: "Not in the library", title, files)
            }
            .sortedBy { it.name }
    }
    // Deleting a saved episode asks first: it is the viewer's own choice to
    // keep it, and getting it back is a download, not a tap.
    var deleting by remember { mutableStateOf<OfflineRecord?>(null) }
    TabPage("Downloads", padding) { body ->
        if (retained.isEmpty() && state.offline.isEmpty()) {
            Box(Modifier.fillMaxSize().padding(body)) {
                EmptyState("No downloads", "Episodes, seasons and shows saved offline appear here.")
            }
            return@TabPage
        }
        LazyColumn(
            modifier = Modifier.fillMaxSize().padding(body),
            contentPadding = PaddingValues(bottom = 24.dp),
        ) {
            if (state.offline.isNotEmpty()) {
                item(key = "summary") {
                    Text(
                        "${state.offline.size} ${if (state.offline.size == 1) "episode" else "episodes"} · " +
                            "${formatBytes(state.offline.sumOf { it.size })} on this device",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
                    )
                }
            }
            // What is still arriving comes first: it is the part of the page
            // that changes, and the part with something to do.
            if (retained.isNotEmpty()) {
                item(key = "in-progress") { SectionHeading("In progress") }
                items(retained, key = { "partial/${it.shareId}/${it.linkId}" }) { download ->
                    PartialRow(download, onPause, onResume, onDeletePartial)
                }
            }
            // Under the show they belong to, as on desktop: a saved season is
            // fourteen rows, and fourteen rows of "Episode 3" name nothing.
            groups.forEach { group ->
                item(key = "group/${group.name}") {
                    SectionHeading(
                        group.name,
                        "${group.files.size} ${if (group.files.size == 1) "episode" else "episodes"} · " +
                            formatBytes(group.files.sumOf { it.size }),
                    )
                }
                items(group.files, key = { "${it.shareId}/${it.linkId}" }) { file ->
                    SavedRow(file, group.title, onDelete = { deleting = file })
                }
            }
        }
    }
    deleting?.let { file ->
        AlertDialog(
            onDismissRequest = { deleting = null },
            title = { Text("Delete this download?") },
            text = {
                Text(
                    "${file.episode?.label ?: "The episode"} (${formatBytes(file.size)}) leaves this device. " +
                        "It still streams, and can be downloaded again.",
                )
            },
            confirmButton = {
                AccentButton(onClick = {
                    deleting = null
                    onRemove(file)
                }) { Text("Delete") }
            },
            dismissButton = { TonalButton(onClick = { deleting = null }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun SectionHeading(title: String, caption: String? = null) {
    Column(Modifier.padding(start = 16.dp, end = 16.dp, top = 20.dp, bottom = 4.dp)) {
        Text(title, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
        caption?.let {
            Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

/** A saved episode: its still, what it is, its size, and a way to remove it. */
@Composable
private fun SavedRow(file: OfflineRecord, title: TitleRecord?, onDelete: () -> Unit) {
    val episode = file.episode
    ListItem(
        leadingContent = {
            RemoteArtwork(
                episode?.stillUrl ?: title?.backdropUrl,
                title?.name ?: file.linkId,
                Modifier.width(96.dp).aspectRatio(16f / 9f).clip(MaterialTheme.shapes.small),
                fallback = episode?.thumbnailSource,
                labelled = episode == null,
            )
        },
        headlineContent = {
            Text(episode?.heading(0) ?: file.linkId, maxLines = 2, overflow = TextOverflow.Ellipsis)
        },
        supportingContent = {
            Text(listOfNotNull(episode?.label, formatBytes(file.size)).joinToString(" · "))
        },
        trailingContent = {
            IconButton(onClick = onDelete) {
                Icon(Icons.Outlined.Delete, contentDescription = "Delete download")
            }
        },
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
    )
}

/**
 * A download not yet finished: how far it has got, why it stopped if it did,
 * and pause or resume beside cancel.
 */
@Composable
private fun PartialRow(
    download: RetainedDownload,
    onPause: (RetainedDownload) -> Unit,
    onResume: (RetainedDownload) -> Unit,
    onDeletePartial: (RetainedDownload) -> Unit,
) {
    val active = download.status == RetainedDownload.STATUS_RUNNING ||
        download.status == RetainedDownload.STATUS_QUEUED
    ListItem(
        headlineContent = { Text(download.label, maxLines = 2, overflow = TextOverflow.Ellipsis) },
        supportingContent = {
            Column {
                Text(
                    listOfNotNull(
                        download.status.replaceFirstChar { it.uppercase() },
                        download.total.takeIf { it > 0L }?.let {
                            "${formatBytes(download.downloaded.toULong())} of ${formatBytes(it.toULong())}"
                        },
                    ).joinToString(" · "),
                )
                if (download.total > 0L) {
                    AccentProgress(
                        progress = { (download.downloaded.toFloat() / download.total).coerceIn(0f, 1f) },
                        modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
                    )
                }
                download.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            }
        },
        trailingContent = {
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (active) {
                    IconButton(onClick = { onPause(download) }) {
                        Icon(Icons.Default.Pause, contentDescription = "Pause download")
                    }
                } else {
                    IconButton(onClick = { onResume(download) }) {
                        Icon(Icons.Default.PlayArrow, contentDescription = "Resume download")
                    }
                }
                // What has arrived so far is discarded; nothing finished is.
                IconButton(onClick = { onDeletePartial(download) }) {
                    Icon(Icons.Default.Close, contentDescription = "Cancel and delete what has arrived")
                }
            }
        },
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
    )
}
