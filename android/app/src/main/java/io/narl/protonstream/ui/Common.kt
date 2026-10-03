package io.narl.protonstream.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import uniffi.pstr_android.EpisodeRecord

@Composable
internal fun EmptyState(title: String, body: String) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Text(title, style = MaterialTheme.typography.titleMedium)
            Text(body, style = MaterialTheme.typography.bodyMedium)
        }
    }
}

/**
 * Where Play on a title lands: the most recently played part-watched episode,
 * else the first unwatched one, else the first — the desktop's `next_up`.
 */
internal fun nextUpIndex(playlist: List<EpisodeRecord>): Int =
    playlist.withIndex().filter { it.value.resumeAt != null }.maxByOrNull { it.value.lastPlayed }?.index
        ?: playlist.indexOfFirst { !it.watched }.takeIf { it >= 0 }
        ?: 0

internal fun formatBytes(bytes: ULong): String {
    val value = bytes.toDouble()
    return when {
        value >= 1024 * 1024 * 1024 -> "%.1f GiB".format(value / (1024 * 1024 * 1024))
        value >= 1024 * 1024 -> "%.1f MiB".format(value / (1024 * 1024))
        value >= 1024 -> "%.1f KiB".format(value / 1024)
        else -> "$bytes bytes"
    }
}
