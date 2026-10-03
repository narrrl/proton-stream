package io.narl.protonstream.ui.theme

import androidx.compose.material3.Typography
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import io.narl.protonstream.R
import androidx.compose.ui.unit.sp

/**
 * Inter, the desktop client's face, from the same two files.
 *
 * Only Regular and SemiBold ship, as on the desktop. A `Medium` role resolves
 * to Regular by the usual weight matching rather than to a synthesised bold,
 * which is how the desktop draws it too.
 *
 * Above [ProtonTypography] on purpose: top-level properties initialise in
 * file order, and a ramp built first would capture this as null and fall back
 * to the system face without a word.
 */
internal val Inter = FontFamily(
    Font(R.font.inter_regular, FontWeight.Normal),
    Font(R.font.inter_semibold, FontWeight.SemiBold),
)

/**
 * The type ramp, which is the desktop client's type ramp.
 *
 * `pstr_app::theme::Role` names nine sizes and the desktop asks for a role
 * rather than a number at every call site. Material names fifteen styles, so
 * several of them land on the same rung — that is the point. The two clients
 * were built to share one palette; sharing one scale is the other half of why
 * they should read as one app.
 *
 * The rungs, largest first: 26 / 20 / 18 / 17 / 15 / 14 / 13 / 12 / 11.
 *
 * Where a role maps is decided by what actually draws with it here, not by
 * Material's own sizes — Material's `displayLarge` is 57sp, which is a size for
 * a phone's lock screen and not for anything in a library grid. `bodyLarge` is
 * what an unstyled `Text` gets, so it takes the desktop's body rung and the two
 * smaller body styles step down from there.
 *
 * [TypeRampTest] pins every one of these against the Rust ramp.
 */
internal val ProtonTypography = Typography(
    displayLarge = role(DISPLAY, FontWeight.SemiBold),
    displayMedium = role(DISPLAY, FontWeight.SemiBold),
    displaySmall = role(TITLE, FontWeight.SemiBold),
    // The name of the title you opened — the desktop's display line.
    headlineLarge = role(DISPLAY, FontWeight.SemiBold),
    headlineMedium = role(DISPLAY, FontWeight.SemiBold),
    // A dialog's heading — `AlertDialog` titles draw with this one.
    headlineSmall = role(HEADING, FontWeight.SemiBold),
    // A page heading: "Offline downloads", "Settings".
    titleLarge = role(TITLE, FontWeight.SemiBold),
    // A run of content under a heading: "Continue watching", "Playback".
    titleMedium = role(SECTION, FontWeight.Medium),
    // A row that groups the rows under it — a season header.
    titleSmall = role(SUBHEAD, FontWeight.Medium),
    bodyLarge = role(BODY, FontWeight.Normal),
    bodyMedium = role(LABEL, FontWeight.Normal),
    // Context rather than content: counts, sizes, the note under a field.
    bodySmall = role(CAPTION, FontWeight.Normal),
    // The text on a button.
    labelLarge = role(LABEL, FontWeight.Medium),
    labelMedium = role(CAPTION, FontWeight.Medium),
    // The smallest thing that stays legible.
    labelSmall = role(MICRO, FontWeight.Medium),
)

internal const val DISPLAY = 26f
internal const val TITLE = 20f
internal const val HEADING = 18f
internal const val SECTION = 17f
internal const val SUBHEAD = 15f
internal const val BODY = 14f
internal const val LABEL = 13f
internal const val CAPTION = 12f
internal const val MICRO = 11f

/**
 * One rung as a `TextStyle`.
 *
 * Line height is derived rather than named per style: a ramp whose leading was
 * chosen nine separate times is the same problem the sizes had. 1.35 is loose
 * enough for the two-line title names in the grid and tight enough that a
 * label on a control does not float.
 */
private fun role(size: Float, weight: FontWeight) = TextStyle(
    fontSize = size.sp,
    lineHeight = (size * 1.35f).sp,
    fontWeight = weight,
    fontFamily = Inter,
)
