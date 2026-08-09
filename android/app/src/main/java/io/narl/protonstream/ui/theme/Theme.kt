package io.narl.protonstream.ui.theme

import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.graphics.Color
import io.narl.protonstream.native.NativeRuntime
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.withContext
import uniffi.pstr_android.PaletteRecord

/**
 * The palette in force, shared across every composition.
 *
 * The choice is stored by Rust and resolved by Rust — the same flavours, the
 * same accent pairings and the same contrast rule the desktop client uses — so
 * what lives here is only the resolved answer and a way to replace it when the
 * viewer picks something else.
 */
object AppearanceState {
    private val current = MutableStateFlow<PaletteRecord?>(null)
    val palette: StateFlow<PaletteRecord?> = current.asStateFlow()

    /**
     * The palette read before the first frame, from
     * [io.narl.protonstream.native.NativeRuntime.storedPalette].
     *
     * Separate from [load] because it runs on the main thread during
     * `onCreate`: it is the only read that is early enough to decide what the
     * first composition is painted in, and it is cheap enough to be allowed
     * there.
     */
    fun seed(palette: PaletteRecord) {
        current.value = palette
    }

    /**
     * Read the stored choice through the engine. Safe to call repeatedly; the
     * last read wins.
     *
     * A no-op once [seed] has run — it resolves the same file through the same
     * Rust, so the only thing a second read can do is build the engine earlier
     * than the first screen needed it.
     */
    suspend fun load() {
        if (current.value != null) return
        runCatching { withContext(Dispatchers.IO) { NativeRuntime.engine().palette() } }
            .onSuccess { current.value = it }
    }

    /** Repaint immediately, before the write that stores the choice lands. */
    fun apply(palette: PaletteRecord) {
        current.value = palette
    }
}

@Composable
fun ProtonStreamTheme(content: @Composable () -> Unit) {
    val palette by AppearanceState.palette.collectAsState()
    // Before the first read, and if it fails, the palette this app shipped
    // with — a theme file that cannot be read must never be a reason the
    // library does not open.
    LaunchedEffect(Unit) { AppearanceState.load() }
    MaterialTheme(
        colorScheme = schemeOf(palette ?: ProtonFallback),
        // The other two thirds of what the clients share. Colour was already
        // one resolved answer for both; type and shape were Material's defaults
        // here and named constants there, which is most of why the two read as
        // different apps even where the colours agreed.
        typography = ProtonTypography,
        shapes = ProtonShapes,
        content = content,
    )
}

/**
 * One resolved palette as Material's roles — **all** of them.
 *
 * Every role Material names has to be answered here, because a role left out
 * does not fall back to something neutral: it keeps the value
 * `darkColorScheme()` / `lightColorScheme()` gave it, which is Material's own
 * baseline purple-and-grey and belongs to no flavour this app ships.
 *
 * That is not hypothetical. `surfaceContainerHighest` is what `Card` fills with
 * (`FilledCardTokens.ContainerColor`), and while it was unmapped every tile in
 * the library grid, every episode row, every share and every download was drawn
 * in baseline neutral on top of a flavoured page. `inverseSurface` is the
 * snackbar — the app's only error channel — and `scrim` is the dim behind every
 * dialog.
 *
 * Material names more roles than a palette has colours, so several are answered
 * by the same one, and two (`elevated`, `dangerDim`) are derived in Rust so the
 * blend behind them is the shared one. `SchemeRolesTest` asserts that nothing
 * here is left at its baseline.
 */
internal fun schemeOf(palette: PaletteRecord): ColorScheme {
    val base = if (palette.light) lightColorScheme() else darkColorScheme()
    return base.copy(
        primary = color(palette.accent),
        onPrimary = color(palette.onAccent),
        primaryContainer = color(palette.accentDim),
        onPrimaryContainer = color(palette.text),
        // Readable *on* `inverseSurface`, which is the ink colour: `accentDim`
        // is the accent taken towards the page, so it inverts with the rest.
        inversePrimary = color(palette.accentDim),
        secondary = color(palette.accentAlt),
        onSecondary = color(palette.onAccent),
        // The accent taken back towards the page, not the card's hover shade.
        // `secondaryContainer` is what fills every tonal button and the
        // navigation bar's selected pill, and `cardHover` sits one step off
        // `card` — which on a dark flavour made all of them dark grey shapes on
        // a dark grey row, readable as text but not as controls.
        secondaryContainer = color(palette.accentDim),
        onSecondaryContainer = color(palette.text),
        // The palette has two hues, not three. Tertiary takes the second one —
        // the gradient partner — rather than inventing a third that no flavour
        // chose.
        tertiary = color(palette.accentAlt),
        onTertiary = color(palette.onAccent),
        tertiaryContainer = color(palette.accentDim),
        onTertiaryContainer = color(palette.text),
        background = color(palette.background),
        onBackground = color(palette.text),
        surface = color(palette.surface),
        onSurface = color(palette.text),
        surfaceVariant = color(palette.card),
        onSurfaceVariant = color(palette.muted),
        // No elevation tint. Material tints a raised surface with `primary`,
        // and the desktop draws no such tint at all — its rule is that the
        // accent is the only strong colour in the window and everything wearing
        // it is something to click. A card that is faintly accent-coloured
        // because it happens to be raised breaks that.
        surfaceTint = Color.Transparent,
        // Genuinely inverted, both ways round: on a dark flavour this is a pale
        // snackbar with dark ink, on Latte a dark one with pale ink.
        inverseSurface = color(palette.text),
        inverseOnSurface = color(palette.background),
        error = color(palette.danger),
        onError = color(palette.background),
        errorContainer = color(palette.dangerDim),
        onErrorContainer = color(palette.text),
        // Swapped from what was here before, to match what Material draws with
        // each: `outline` is a border meant to be seen (an outlined button,
        // a swatch that is not selected), `outlineVariant` is a divider meant
        // to be barely there. `border` is the fainter of the two.
        outline = color(palette.muted),
        outlineVariant = color(palette.border),
        // Black in both of Material's own schemes, and for the same reason: a
        // scrim is not a colour, it is an absence of light.
        scrim = Color.Black,
        surfaceBright = color(palette.elevated),
        surfaceDim = color(palette.background),
        // The containment ladder, in the order the palette already runs: the
        // deepest colour in the flavour at the bottom, `elevated` — one step
        // past the card's hover shade — at the top.
        surfaceContainerLowest = color(palette.sunken),
        surfaceContainerLow = color(palette.surface),
        surfaceContainer = color(palette.card),
        surfaceContainerHigh = color(palette.cardHover),
        surfaceContainerHighest = color(palette.elevated),
        // The fixed roles are Material's "the same in light and dark", which a
        // per-flavour palette has no equivalent of. They take the accent pair
        // rather than a baseline hue, so a component that reaches for one still
        // lands inside the flavour.
        primaryFixed = color(palette.accentDim),
        primaryFixedDim = color(palette.accent),
        onPrimaryFixed = color(palette.text),
        onPrimaryFixedVariant = color(palette.muted),
        secondaryFixed = color(palette.accentDim),
        secondaryFixedDim = color(palette.accentAlt),
        onSecondaryFixed = color(palette.text),
        onSecondaryFixedVariant = color(palette.muted),
        tertiaryFixed = color(palette.accentDim),
        tertiaryFixedDim = color(palette.accentAlt),
        onTertiaryFixed = color(palette.text),
        onTertiaryFixedVariant = color(palette.muted),
    )
}

/** A packed `0xAARRGGBB` from the bridge, as Compose wants it. */
private fun color(argb: UInt) = Color(argb.toInt())

/**
 * The shipped default, spelled out for the case where Rust cannot be asked.
 *
 * A `PaletteRecord` rather than a `ColorScheme`, so that it goes through
 * [schemeOf] like any other palette and inherits every mapping above — a
 * fallback that answered only ten roles would be the same bug in a smaller
 * place. The values are `pstr_core::appearance::Palette::PROTON`.
 */
internal val ProtonFallback = PaletteRecord(
    background = 0xFF0E0E12u,
    surface = 0xFF17171Du,
    sunken = 0xFF0A0A0Du,
    card = 0xFF1E1E26u,
    cardHover = 0xFF2A2A35u,
    border = 0xFF262630u,
    text = 0xFFEAEAF0u,
    muted = 0xFF8E8E9Cu,
    accent = 0xFF7D4DFFu,
    accentAlt = 0xFF3F6FE0u,
    accentDim = 0xFF5333ADu,
    onAccent = 0xFFEAEAF0u,
    danger = 0xFFE05561u,
    elevated = 0xFF333340u,
    dangerDim = 0xFF7F2F37u,
    light = false,
)
