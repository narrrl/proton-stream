package io.narl.protonstream.ui.theme

import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Shapes
import androidx.compose.ui.unit.dp

/**
 * The suite's corner radii (`docs/DESIGN-TOKENS.md`), which are the desktop
 * client's: `theme::radius` in pstr-app has the same numbers.
 *
 * Material's default ramp runs 4 / 8 / 12 / 16 / 28, so a stock Compose screen
 * puts a 28dp bottom sheet next to a 12dp card, and neither is a shape the
 * other client draws. Flatter than that, therefore: cards and tiles at 12,
 * badges and chips at 6, and a sheet or a dialog at 16, which floats.
 * Buttons are pills, [Pill], as on the desktop and the suite's websites.
 */
internal val ProtonShapes = Shapes(
    extraSmall = RoundedCornerShape(CORNER_TIGHT.dp),
    small = RoundedCornerShape(CORNER_SMALL.dp),
    medium = RoundedCornerShape(CORNER.dp),
    large = RoundedCornerShape(CORNER_SHEET.dp),
    extraLarge = RoundedCornerShape(CORNER_SHEET.dp),
)

/**
 * What a tile with its caption under the picture clips its press ripple to:
 * the card's corners on top, tight ones under the text. Clipped to [CORNER]
 * all round, the bottom-left curve cut into the first letter of the caption's
 * last line.
 */
internal val TileShape = RoundedCornerShape(
    topStart = CORNER.dp,
    topEnd = CORNER.dp,
    bottomStart = CORNER_TIGHT.dp,
    bottomEnd = CORNER_TIGHT.dp,
)

/** Cards, tiles, rows: `theme::radius::MD`. */
internal const val CORNER = 12f

/** A small control inside something else, a text field: `theme::radius::SM`. */
internal const val CORNER_SMALL = 8f

/** A badge over artwork, a chip: `theme::radius::BADGE`. */
internal const val CORNER_TIGHT = 6f

/** A sheet, a dialog, the featured banner: `theme::radius::LG`. */
internal const val CORNER_SHEET = 16f

/** Something that floats over the page: `theme::radius::FLOAT`. */
internal const val CORNER_FLOAT = 24f

/** Buttons and the navigation pill: half the height, whatever the height. */
internal val Pill = RoundedCornerShape(percent = 50)
