package io.narl.protonstream.ui.tv

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.BringIntoViewSpec
import androidx.compose.foundation.gestures.LocalBringIntoViewSpec
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.focusRestorer
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.tv.material3.Border
import androidx.tv.material3.Card
import androidx.tv.material3.CardDefaults
import androidx.tv.material3.Icon
import androidx.tv.material3.ListItem
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.Surface
import androidx.tv.material3.SurfaceDefaults
import androidx.tv.material3.Text
import io.narl.protonstream.ui.Backdrop
import io.narl.protonstream.ui.RemoteArtwork
import io.narl.protonstream.ui.onMenuKey
import io.narl.protonstream.ui.returnsFocus
import io.narl.protonstream.ui.theme.AccentProgress
import io.narl.protonstream.ui.thumbnailSource
import io.narl.protonstream.ui.tryFocus
import uniffi.pstr_android.EpisodeRecord
import uniffi.pstr_android.TitleRecord

/**
 * Android TV's overscan-safe margins: five percent of a 960×540 dp screen.
 * A set may cut off its edges, so nothing that matters is drawn outside them.
 */
internal val TV_SIDE = 48.dp
internal val TV_TOP = 27.dp

/** Six to seven posters to a row on a 960 dp screen, a row being one glance. */
internal val POSTER_WIDTH = 124.dp

/** Continue watching: a frame of the episode, wide enough to read across a room. */
internal val WIDE_WIDTH = 248.dp

/** tv-material's own card corners, which the outline has to follow. */
private val CARD_SHAPE = RoundedCornerShape(8.dp)

/** How much a focused card grows: noticeable, without hiding its neighbours. */
private const val FOCUSED_SCALE = 1.08f

/** The outline a focused card wears: the accent, as on the phone's D-pad focus. */
@Composable
internal fun focusBorder(shape: Shape = CARD_SHAPE) = Border(
    border = BorderStroke(3.dp, MaterialTheme.colorScheme.border),
    shape = shape,
)

/**
 * One title as a poster, its name under it.
 *
 * [onFocus] tells the page which title is in front, which the backdrop follows.
 * Menu, or holding OK, opens [onMenu]: what a long press opens on a phone.
 */
@Composable
internal fun PosterCard(
    title: TitleRecord,
    onClick: () -> Unit,
    onMenu: () -> Unit,
    modifier: Modifier = Modifier,
    onFocus: (TitleRecord) -> Unit = {},
    badge: String? = null,
) {
    Column(modifier.width(POSTER_WIDTH)) {
        Card(
            onClick = onClick,
            onLongClick = onMenu,
            modifier = Modifier
                .fillMaxWidth()
                .aspectRatio(2f / 3f)
                .returnsFocus(title.key)
                .onMenuKey(onMenu)
                .onFocusChanged { if (it.isFocused) onFocus(title) },
            scale = CardDefaults.scale(focusedScale = FOCUSED_SCALE),
            border = CardDefaults.border(focusedBorder = focusBorder()),
        ) {
            Box(Modifier.fillMaxSize()) {
                RemoteArtwork(
                    title.posterUrl,
                    title.displayName,
                    Modifier.fillMaxSize().clearAndSetSemantics {},
                    fallback = title.thumbnailSource,
                    maxEdge = POSTER_WIDTH,
                )
                badge?.let { Badge(it, Modifier.align(Alignment.TopStart).padding(6.dp)) }
            }
        }
        Text(
            title.displayName,
            style = MaterialTheme.typography.labelLarge,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(top = 8.dp),
        )
    }
}

/**
 * A wide card: a frame of the episode, how far through it is, and its name.
 * Continue watching, where what is shown is an episode rather than a show.
 */
@Composable
internal fun EpisodeCard(
    title: TitleRecord,
    episode: EpisodeRecord,
    caption: String,
    onClick: () -> Unit,
    onMenu: () -> Unit,
    modifier: Modifier = Modifier,
    onFocus: (TitleRecord) -> Unit = {},
) {
    Column(modifier.width(WIDE_WIDTH)) {
        Card(
            onClick = onClick,
            onLongClick = onMenu,
            modifier = Modifier
                .fillMaxWidth()
                .aspectRatio(16f / 9f)
                .onMenuKey(onMenu)
                .onFocusChanged { if (it.isFocused) onFocus(title) },
            scale = CardDefaults.scale(focusedScale = FOCUSED_SCALE),
            border = CardDefaults.border(focusedBorder = focusBorder()),
        ) {
            Box(Modifier.fillMaxSize()) {
                RemoteArtwork(
                    episode.stillUrl ?: title.backdropUrl,
                    title.displayName,
                    Modifier.fillMaxSize().clearAndSetSemantics {},
                    fallback = episode.thumbnailSource,
                    maxEdge = WIDE_WIDTH,
                )
                episode.progress?.let { progress ->
                    AccentProgress(
                        progress = { progress.toFloat() },
                        modifier = Modifier.align(Alignment.BottomCenter).padding(horizontal = 8.dp, vertical = 8.dp),
                    )
                }
            }
        }
        Text(
            title.displayName,
            style = MaterialTheme.typography.labelLarge,
            fontWeight = FontWeight.SemiBold,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(top = 8.dp),
        )
        Text(
            caption,
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** A short label in a card's corner: "New", "Ep 8 · in 2 d". */
@Composable
private fun Badge(text: String, modifier: Modifier = Modifier) {
    Text(
        text,
        style = MaterialTheme.typography.labelSmall,
        color = MaterialTheme.colorScheme.onPrimary,
        maxLines = 1,
        modifier = modifier
            .background(MaterialTheme.colorScheme.primary, RoundedCornerShape(4.dp))
            .padding(horizontal = 6.dp, vertical = 2.dp),
    )
}

/**
 * A heading and a row of cards under it.
 *
 * Coming back into the row, from above or below, lands on the card that last
 * had the focus rather than the first: the row remembers where it was, as a
 * television's own launcher does. [state] is kept by the caller so the row is
 * still scrolled where it was after a title page has been open over it.
 */
@OptIn(ExperimentalComposeUiApi::class)
@Composable
internal fun CardRow(
    heading: String,
    modifier: Modifier = Modifier,
    state: LazyListState = rememberLazyListState(),
    content: LazyListScope.() -> Unit,
) {
    Column(modifier) {
        Text(
            heading,
            style = MaterialTheme.typography.titleMedium,
            modifier = Modifier.padding(start = TV_SIDE, bottom = 4.dp),
        )
        ScrollsTo(TV_SIDE) {
            LazyRow(
                state = state,
                modifier = Modifier.focusRestorer(),
                // Room around the cards for one to grow into when it is focused.
                contentPadding = PaddingValues(horizontal = TV_SIDE, vertical = 12.dp),
                horizontalArrangement = Arrangement.spacedBy(20.dp),
                content = content,
            )
        }
    }
}

/**
 * Lists in [content] scroll what the focus moves to [lead] from their start, the
 * way a television's launcher lines its rows up: down the page, the focused row
 * sits under the top with its heading showing; along a row, the focused card
 * stays at the left margin.
 *
 * Compose's own choice on a television (`PivotBringIntoViewSpec`) puts it a third
 * of the way in instead, which on a page of rows leaves half of the row above
 * cut off between the top and the one focused.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
internal fun ScrollsTo(lead: Dp, content: @Composable () -> Unit) {
    val px = with(LocalDensity.current) { lead.toPx() }
    val spec = remember(px) {
        object : BringIntoViewSpec {
            override fun calculateScrollDistance(offset: Float, size: Float, containerSize: Float): Float = offset - px
        }
    }
    CompositionLocalProvider(LocalBringIntoViewSpec provides spec, content = content)
}

/**
 * Lists in [content] scroll only as far as it takes to show what the focus moved
 * to, and [margin] past it, so a poster's name under it and a glimpse of the next
 * row come into view too. For a page whose top must stay put while its first
 * control has the focus, as a title's page keeps its name over Play, and for a
 * grid, where lining every row up at the top would hide the one just left.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
internal fun ScrollsJustEnough(margin: Dp = SCROLL_MARGIN, content: @Composable () -> Unit) {
    val px = with(LocalDensity.current) { margin.toPx() }
    val spec = remember(px) { JustEnough(px) }
    CompositionLocalProvider(LocalBringIntoViewSpec provides spec, content = content)
}

@OptIn(ExperimentalFoundationApi::class)
private class JustEnough(private val margin: Float) : BringIntoViewSpec {
    override fun calculateScrollDistance(offset: Float, size: Float, containerSize: Float): Float {
        val start = offset - margin
        val end = offset + size + margin
        return when {
            start >= 0 && end <= containerSize -> 0f
            // Taller than the page with its margins, or above it: its top.
            end - start > containerSize || start < 0 -> start
            else -> end - containerSize
        }
    }
}

private val SCROLL_MARGIN = 72.dp

/** How far under the top of a page the focused row sits: room for its heading. */
internal val ROW_LEAD = 44.dp

/**
 * A title's backdrop behind a page, fading into the page's background to the
 * left, where the text is, and towards the bottom, where the rows are.
 */
@Composable
internal fun BoxScope.PageBackdrop(title: TitleRecord?, height: Float = 0.8f) {
    val background = MaterialTheme.colorScheme.background
    Box(
        Modifier
            .align(Alignment.TopEnd)
            .fillMaxWidth(0.78f)
            .fillMaxHeight(height),
    ) {
        title?.let { Backdrop(it, Modifier.fillMaxSize()) }
        Box(
            Modifier.fillMaxSize().background(
                Brush.horizontalGradient(0f to background, 0.45f to background.copy(alpha = 0.55f), 1f to Color.Transparent),
            ),
        )
        Box(
            Modifier.fillMaxSize().background(
                Brush.verticalGradient(0.45f to Color.Transparent, 1f to background),
            ),
        )
    }
}

/** One entry of a [TvMenu]. */
internal class MenuEntry(val icon: ImageVector, val label: String, val onClick: () -> Unit)

/**
 * What a long press opens on a phone, as a list a remote can move through: the
 * first entry is focused when it opens, Back closes it.
 */
@Composable
internal fun TvMenu(heading: String, detail: String?, entries: List<MenuEntry>, onDismiss: () -> Unit) {
    Dialog(onDismissRequest = onDismiss) {
        Surface(
            shape = RoundedCornerShape(16.dp),
            colors = SurfaceDefaults.colors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
            modifier = Modifier.width(MENU_WIDTH),
        ) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(heading, style = MaterialTheme.typography.titleLarge, maxLines = 2, overflow = TextOverflow.Ellipsis)
                detail?.let {
                    Text(
                        it,
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.padding(bottom = 8.dp),
                    )
                }
                val first = remember { FocusRequester() }
                entries.forEachIndexed { index, entry ->
                    ListItem(
                        selected = false,
                        onClick = {
                            onDismiss()
                            entry.onClick()
                        },
                        headlineContent = { Text(entry.label) },
                        leadingContent = { Icon(entry.icon, contentDescription = null) },
                        modifier = if (index == 0) Modifier.focusRequester(first) else Modifier,
                    )
                }
                LaunchedEffect(Unit) { first.tryFocus() }
            }
        }
    }
}

private val MENU_WIDTH = 420.dp
