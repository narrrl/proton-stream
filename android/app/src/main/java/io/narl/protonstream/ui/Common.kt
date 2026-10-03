package io.narl.protonstream.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarColors
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.nestedscroll.nestedScroll
import uniffi.pstr_android.EpisodeRecord

/**
 * A tab's page: its own name in an app bar, over whatever the page holds.
 *
 * One bar for the whole app said "proton-stream" on every page, which is a band
 * of height spent naming the app the viewer is already in. The bar draws under
 * the status bar itself, which is why the shell hands pages no top padding;
 * [content] gets the rest of [padding] (the navigation bar and the mini
 * transport) to apply as it scrolls.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun TabPage(
    title: String,
    padding: PaddingValues,
    navigationIcon: @Composable () -> Unit = {},
    actions: @Composable RowScope.() -> Unit = {},
    content: @Composable (PaddingValues) -> Unit,
) {
    // Pinned: the bar stays, and takes the container tint once the page has
    // scrolled under it, so the edge between the two is still visible.
    val scroll = TopAppBarDefaults.pinnedScrollBehavior()
    Column(Modifier.fillMaxSize().nestedScroll(scroll.nestedScrollConnection)) {
        TopAppBar(
            title = { Text(title) },
            navigationIcon = navigationIcon,
            actions = actions,
            colors = pageBarColors(),
            scrollBehavior = scroll,
        )
        Box(Modifier.weight(1f)) { content(padding) }
    }
}

/**
 * The page's own background until something scrolls under the bar, then the
 * container tint. A bar in `surface` from the start read as a band across the
 * top of a page drawn in `background`.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun pageBarColors(): TopAppBarColors = TopAppBarDefaults.topAppBarColors(
    containerColor = MaterialTheme.colorScheme.background,
    scrolledContainerColor = MaterialTheme.colorScheme.surfaceContainer,
)

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
