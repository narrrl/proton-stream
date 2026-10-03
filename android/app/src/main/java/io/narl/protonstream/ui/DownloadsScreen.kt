package io.narl.protonstream.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.Icon
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Delete
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.livedata.observeAsState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.work.WorkManager
import io.narl.protonstream.download.DownloadCoordinator
import io.narl.protonstream.download.DownloadStateStore
import io.narl.protonstream.download.RetainedDownload
import io.narl.protonstream.ui.theme.AccentProgress
import io.narl.protonstream.ui.theme.QuietButton
import io.narl.protonstream.ui.theme.TonalButton
import uniffi.pstr_android.SeasonRecord

@Composable
internal fun DownloadsScreen(
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
    TabPage("Downloads", padding) { body ->
        LazyColumn(
            modifier = Modifier.fillMaxSize().padding(body).padding(horizontal = 16.dp),
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
}
