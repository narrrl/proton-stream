package io.narl.protonstream.ui

import androidx.compose.animation.AnimatedVisibilityScope
import androidx.compose.animation.ExperimentalSharedTransitionApi
import androidx.compose.animation.SharedTransitionScope
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
import androidx.compose.runtime.staticCompositionLocalOf
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

/**
 * The shared-element scopes the library-to-title switch runs in, or null where
 * there is none — the two-pane layout, a screenshot, a preview. Locals rather
 * than parameters, because the poster that animates is three calls deep in the
 * grid and nothing between needs to know.
 */
@OptIn(ExperimentalSharedTransitionApi::class)
internal val LocalSharedTransition = staticCompositionLocalOf<SharedTransitionScope?> { null }
internal val LocalPaneVisibility = staticCompositionLocalOf<AnimatedVisibilityScope?> { null }

/**
 * A title's art as one element across the switch: the poster tapped in the
 * grid grows into the backdrop of the page it opens, and shrinks back on the
 * way out, so the page is visibly *that* title's rather than a new screen.
 */
@OptIn(ExperimentalSharedTransitionApi::class)
@Composable
internal fun sharedArt(key: String): Modifier {
    val transition = LocalSharedTransition.current ?: return Modifier
    val visibility = LocalPaneVisibility.current ?: return Modifier
    return with(transition) {
        Modifier.sharedBounds(rememberSharedContentState("art/$key"), visibility)
    }
}

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

/**
 * What an episode row is headed with: "1. Mother and Children".
 *
 * The provider's name for the episode, else the name the filename gave it,
 * else "Episode 1". The bare filename only when none of those exist — a column
 * of `[Group] Show - S01E01.mkv` is forty rows that differ in two characters.
 */
internal fun EpisodeRecord.heading(position: Int): String {
    val named = providerName ?: detail.takeIf { it != name }
    val number = number?.toInt()
    return when {
        named != null && number != null -> "$number. $named"
        named != null -> named
        number != null -> "Episode $number"
        else -> name.substringBeforeLast('.').ifBlank { "Episode ${position + 1}" }
    }
}
