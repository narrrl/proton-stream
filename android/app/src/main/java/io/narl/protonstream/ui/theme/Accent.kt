package io.narl.protonstream.ui.theme

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.NavigationDrawerItemDefaults
import androidx.compose.material3.NavigationRailItemDefaults
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.adaptive.navigationsuite.NavigationSuiteDefaults
import androidx.compose.material3.adaptive.navigationsuite.NavigationSuiteItemColors
import androidx.compose.material3.SliderState
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.unit.dp

/**
 * The accent, as a gradient when there is one to draw.
 *
 * The desktop paints the accent as a two-hue ramp on everything that is a
 * control — the seek bar, the primary buttons, the tile progress bar — and the
 * viewer's "paint the accent as a gradient" toggle is what turns it off. That
 * toggle was already stored, already offered on this screen, and drawn nowhere
 * on Android, so the same stored setting meant two different things depending
 * on which client read it.
 *
 * Nothing here reads the toggle. It does not need to: when gradients are off,
 * `Palette::resolve` sets `accent_alt` equal to `accent`, so the ramp collapses
 * to a flat fill on its own. `primary` and `secondary` are those two colours —
 * see [schemeOf] — which is why this asks Material for them rather than
 * reaching past it for the palette.
 */
@Composable
internal fun accentBrush(): Brush {
    val scheme = MaterialTheme.colorScheme
    return remember(scheme.primary, scheme.secondary) {
        Brush.horizontalGradient(listOf(scheme.primary, scheme.secondary))
    }
}

/**
 * The primary button: the accent ramp, at the shared corner radius.
 *
 * Material's filled button has its own shape token (`CornerFull`) that does not
 * read `MaterialTheme.shapes`, so the radius has to be handed to it — which is
 * the whole reason the plain buttons below exist as well. They add nothing but
 * that.
 */
@Composable
internal fun AccentButton(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    content: @Composable RowScope.() -> Unit,
) {
    val shape = MaterialTheme.shapes.small
    Button(
        onClick = onClick,
        // Only while it is live: a disabled button keeps Material's flat
        // disabled container, because a greyed control that still wears the
        // one strong colour in the window reads as pressable.
        modifier = if (enabled) modifier.background(accentBrush(), shape) else modifier,
        enabled = enabled,
        shape = shape,
        colors = ButtonDefaults.buttonColors(containerColor = Color.Transparent),
        content = content,
    )
}

@Composable
internal fun TonalButton(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    content: @Composable RowScope.() -> Unit,
) = FilledTonalButton(
    onClick = onClick,
    modifier = modifier,
    enabled = enabled,
    shape = MaterialTheme.shapes.small,
    content = content,
)

@Composable
internal fun EdgedButton(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    content: @Composable RowScope.() -> Unit,
) = OutlinedButton(
    onClick = onClick,
    modifier = modifier,
    enabled = enabled,
    shape = MaterialTheme.shapes.small,
    content = content,
)

@Composable
internal fun QuietButton(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    content: @Composable RowScope.() -> Unit,
) = TextButton(
    onClick = onClick,
    modifier = modifier,
    enabled = enabled,
    shape = MaterialTheme.shapes.small,
    content = content,
)

/**
 * How far along something is, drawn in the accent.
 *
 * Hand-drawn rather than `LinearProgressIndicator` because that one takes a
 * `Color` and not a `Brush`, and because Material 1.4 draws a gap and a stop
 * indicator either side of the bar — detail at a size where the desktop draws a
 * plain bar. [fill] is a parameter for the one bar that is deliberately not the
 * accent: a download in flight sits directly under the watch-progress bar of
 * the same row, and two ramps in the same colour stacked two pixels apart say
 * nothing.
 */
@Composable
internal fun AccentProgress(
    progress: () -> Float,
    modifier: Modifier = Modifier,
    fill: Brush? = null,
) {
    val brush = fill ?: accentBrush()
    val track = MaterialTheme.colorScheme.surfaceContainerHighest
    Canvas(modifier.fillMaxWidth().height(PROGRESS_HEIGHT.dp)) {
        val radius = CornerRadius(size.height / 2)
        drawRoundRect(color = track, cornerRadius = radius)
        val done = size.width * progress().coerceIn(0f, 1f)
        // Below one bar's width there is nothing to round, and a rounded rect
        // narrower than its own corners draws as a wrong-looking pill.
        if (done >= size.height) {
            drawRoundRect(brush = brush, size = Size(done, size.height), cornerRadius = radius)
        }
    }
}

/**
 * The seek bar's track, in the accent ramp.
 *
 * Passed to `Slider`'s `track` slot, so the thumb, the gestures and the
 * accessibility semantics stay Material's — this replaces the two rectangles it
 * would have drawn and nothing else.
 *
 * The spent side is the accent and the rest is white at a third: the seek bar
 * is the one accent surface that sits over arbitrary film footage, where a
 * palette colour for the *unplayed* remainder would be a coloured line over a
 * picture rather than the absence of one.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun AccentTrack(state: SliderState, modifier: Modifier = Modifier, buffered: Float = 0f) {
    val brush = accentBrush()
    Canvas(modifier.fillMaxWidth().height(TRACK_HEIGHT.dp)) {
        val radius = CornerRadius(size.height / 2)
        drawRoundRect(color = Color.White.copy(alpha = 0.30f), cornerRadius = radius)
        // What is already read ahead, lighter than the rest and under the
        // played part: a jump that lands in it costs no fetch.
        val ahead = size.width * buffered.coerceIn(0f, 1f)
        if (ahead >= size.height) {
            drawRoundRect(
                color = Color.White.copy(alpha = 0.55f),
                size = Size(ahead, size.height),
                cornerRadius = radius,
            )
        }
        val done = size.width * state.coercedValueAsFraction
        if (done >= size.height) {
            drawRoundRect(brush = brush, size = Size(done, size.height), cornerRadius = radius)
        }
    }
}

/**
 * The navigation pill, in the accent.
 *
 * Solid rather than a ramp, and this is the one place the two clients cannot
 * agree exactly: `NavigationSuiteScaffold` takes the indicator as a `Color`,
 * with no slot to draw into, so a gradient here would mean reimplementing the
 * bar. The accent itself is still the right answer — the desktop's rule is that
 * the accent marks the thing you are on — and it is nearer to that than
 * `secondaryContainer`, which is the accent taken most of the way back to the
 * page.
 */
@Composable
internal fun accentNavigationColors(): NavigationSuiteItemColors {
    val scheme = MaterialTheme.colorScheme
    return NavigationSuiteDefaults.itemColors(
        navigationBarItemColors = NavigationBarItemDefaults.colors(
            selectedIconColor = scheme.onPrimary,
            selectedTextColor = scheme.onSurface,
            indicatorColor = scheme.primary,
        ),
        navigationRailItemColors = NavigationRailItemDefaults.colors(
            selectedIconColor = scheme.onPrimary,
            selectedTextColor = scheme.onSurface,
            indicatorColor = scheme.primary,
        ),
        navigationDrawerItemColors = NavigationDrawerItemDefaults.colors(
            selectedContainerColor = scheme.primary,
            selectedIconColor = scheme.onPrimary,
            selectedTextColor = scheme.onPrimary,
        ),
    )
}

/** A solid fill, for the callers of [AccentProgress] that want one colour. */
internal fun solid(color: Color): Brush = SolidColor(color)

/** Thin enough to sit under a row without becoming part of it. */
private const val PROGRESS_HEIGHT = 4f

/** The seek bar is dragged with a finger, so it is the thicker of the two. */
private const val TRACK_HEIGHT = 6f
