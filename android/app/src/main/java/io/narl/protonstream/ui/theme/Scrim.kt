package io.narl.protonstream.ui.theme

import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color

/**
 * `pstr_core::appearance::SCRIM_STOPS`: the suite's one fade, as
 * `(position, opacity)` from the solid end to the clear one.
 *
 * Eased (`1 - smoothstep`), flat at both ends, so a scrim over artwork has no
 * line where it starts or stops. A linear fade does: the eye sees the change
 * of slope where it meets the clear picture. The desktop draws the same
 * curve, and a test there pins these numbers to it.
 */
private val SCRIM_STOPS = floatArrayOf(1f, 0.957f, 0.844f, 0.684f, 0.5f, 0.316f, 0.156f, 0.043f, 0f)

private fun stops(color: Color, from: Float, to: Float, solidAtStart: Boolean): Array<Pair<Float, Color>> {
    val last = SCRIM_STOPS.size - 1
    return Array(SCRIM_STOPS.size) { index ->
        val position = from + (to - from) * index / last
        val opacity = SCRIM_STOPS[if (solidAtStart) index else last - index]
        position to color.copy(alpha = color.alpha * opacity)
    }
}

/**
 * A fade in [color] down the box, between the fractions [from] and [to] of
 * its height. [solidAtTop] says which end is opaque.
 */
internal fun verticalScrim(color: Color, from: Float = 0f, to: Float = 1f, solidAtTop: Boolean = false): Brush =
    Brush.verticalGradient(*stops(color, from, to, solidAtTop))

/** The same across the box, opaque at the start (left) edge when [solidAtStart]. */
internal fun horizontalScrim(color: Color, from: Float = 0f, to: Float = 1f, solidAtStart: Boolean = true): Brush =
    Brush.horizontalGradient(*stops(color, from, to, solidAtStart))
