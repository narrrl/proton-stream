package io.narl.protonstream.ui.theme

import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Shapes
import androidx.compose.ui.unit.dp

/**
 * One corner radius, which is the desktop client's corner radius.
 *
 * `pstr-app` rounds every surface it paints to 8 — cards, buttons, the tab
 * pill, the progress bars — and only a badge over artwork is tighter, at 4.
 * Material's default ramp runs 4 / 8 / 12 / 16 / 28, so a stock Compose screen
 * puts a 28dp bottom sheet next to a 12dp card next to a fully-round button,
 * and none of those is the shape the other client draws.
 *
 * Flat rather than a ramp, therefore, with the two ends kept for the two cases
 * where a single radius is wrong: `extraSmall` for something small enough that
 * 8 would eat it, and `extraLarge` for a sheet whose corners meet the edge of
 * the screen and need to read as a lifted sheet rather than a seam.
 */
internal val ProtonShapes = Shapes(
    extraSmall = RoundedCornerShape(CORNER_TIGHT.dp),
    small = RoundedCornerShape(CORNER.dp),
    medium = RoundedCornerShape(CORNER.dp),
    large = RoundedCornerShape(CORNER.dp),
    extraLarge = RoundedCornerShape(CORNER_SHEET.dp),
)

/** The radius `pstr-app` draws everything at. */
internal const val CORNER = 8f

/** A badge over artwork — `theme::radius::BADGE` in pstr-app is the other place this number lives. */
internal const val CORNER_TIGHT = 4f

/** A bottom sheet, which has no desktop counterpart. */
internal const val CORNER_SHEET = 16f
