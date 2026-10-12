package io.narl.protonstream.ui

import android.net.ConnectivityManager
import androidx.compose.animation.AnimatedVisibilityScope
import androidx.compose.animation.ExperimentalSharedTransitionApi
import androidx.compose.animation.SharedTransitionScope
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarColors
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import io.narl.protonstream.download.RetainedDownload
import io.narl.protonstream.settings.SettingsStore
import io.narl.protonstream.ui.theme.AccentButton
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
internal fun Modifier.sharedArt(key: String): Modifier {
    val transition = LocalSharedTransition.current ?: return this
    val visibility = LocalPaneVisibility.current ?: return this
    return with(transition) {
        this@sharedArt.sharedBounds(rememberSharedContentState("art/$key"), visibility)
    }
}

@Composable
internal fun EmptyState(title: String, body: String, action: Pair<String, () -> Unit>? = null) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Column(Modifier.padding(horizontal = 32.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            Text(title, style = MaterialTheme.typography.titleMedium, textAlign = TextAlign.Center)
            Text(
                body,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
                modifier = Modifier.padding(top = 4.dp),
            )
            action?.let { (label, onClick) ->
                AccentButton(onClick = onClick, modifier = Modifier.padding(top = 16.dp)) { Text(label) }
            }
        }
    }
}

/**
 * Where Play on a title lands: what Continue watching offers
 * ([TitleRecord.continueIndex]), else the most recently played part-watched
 * episode, else the first unwatched one, else the first — the desktop's
 * `next_up`.
 */
internal fun nextUpIndex(playlist: List<EpisodeRecord>, continueIndex: UInt? = null): Int =
    continueIndex?.toInt()?.takeIf { it in playlist.indices }
        ?: playlist.withIndex().filter { it.value.resumeAt != null }.maxByOrNull { it.value.lastPlayed }?.index
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
 * One line under a download that has not finished: "Running · 120.0 MiB of
 * 1.2 GiB · 2.4 MiB/s", or "Queued" while it waits for a slot — or, given a
 * [waitingFor], what it is really waiting on.
 */
internal fun downloadStatusLine(download: RetainedDownload, waitingFor: String? = null): String = listOfNotNull(
    waitingFor?.takeIf { download.status == RetainedDownload.STATUS_QUEUED }
        ?: download.status.replaceFirstChar { it.uppercase() },
    download.total.takeIf { it > 0L && download.downloaded > 0L }?.let {
        "${formatBytes(download.downloaded.toULong())} of ${formatBytes(it.toULong())}"
    },
    download.bytesPerSecond
        .takeIf { it > 0L && download.status == RetainedDownload.STATUS_RUNNING }
        ?.let { "${formatBytes(it.toULong())}/s" },
).joinToString(" · ")

/**
 * Why a queued download is not running, when it is not merely a free slot:
 * WorkManager holds a Wi-Fi-only download back on mobile data without a word,
 * and "Queued" for an hour on the bus reads as stuck. Re-read whenever [key]
 * changes — the download's own updates are the clock.
 */
@Composable
internal fun rememberQueueReason(key: Any?): String? {
    val context = LocalContext.current
    return remember(key) {
        val network = context.getSystemService(ConnectivityManager::class.java)
        when {
            network?.activeNetwork == null -> "Waiting for a connection"
            SettingsStore(context).wifiOnly && network.isActiveNetworkMetered -> "Waiting for Wi-Fi"
            else -> null
        }
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
