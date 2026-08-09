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

    /** Read the stored choice. Safe to call repeatedly; the last read wins. */
    suspend fun load() {
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
    MaterialTheme(colorScheme = palette?.let(::schemeOf) ?: ProtonColors, content = content)
}

/**
 * One resolved palette as Material's roles.
 *
 * Material names more roles than a palette has colours, so several are answered
 * by the same one — `surfaceVariant` and `card` above all, which is what makes
 * a tile and a form field the same shade in both clients.
 */
private fun schemeOf(palette: PaletteRecord): ColorScheme {
    val base = if (palette.light) lightColorScheme() else darkColorScheme()
    return base.copy(
        primary = color(palette.accent),
        onPrimary = color(palette.onAccent),
        primaryContainer = color(palette.accentDim),
        onPrimaryContainer = color(palette.text),
        secondary = color(palette.accentAlt),
        onSecondary = color(palette.onAccent),
        // The accent taken back towards the page, not the card's hover shade.
        // `secondaryContainer` is what fills every tonal button and the
        // navigation bar's selected pill, and `cardHover` sits one step off
        // `card` — which on a dark flavour made all of them dark grey shapes on
        // a dark grey row, readable as text but not as controls.
        secondaryContainer = color(palette.accentDim),
        onSecondaryContainer = color(palette.text),
        background = color(palette.background),
        onBackground = color(palette.text),
        surface = color(palette.surface),
        onSurface = color(palette.text),
        surfaceVariant = color(palette.card),
        onSurfaceVariant = color(palette.muted),
        surfaceContainer = color(palette.card),
        surfaceContainerHigh = color(palette.cardHover),
        surfaceContainerLow = color(palette.sunken),
        outline = color(palette.border),
        outlineVariant = color(palette.muted),
        error = color(palette.danger),
        onError = color(palette.background),
    )
}

/** A packed `0xAARRGGBB` from the bridge, as Compose wants it. */
private fun color(argb: UInt) = Color(argb.toInt())

/** The shipped default, spelled out for the frames before the store is read. */
private val ProtonColors = darkColorScheme(
    primary = Color(0xFF7D4DFF),
    onPrimary = Color(0xFFEAEAF0),
    secondary = Color(0xFF3F6FE0),
    background = Color(0xFF0E0E12),
    surface = Color(0xFF17171D),
    surfaceVariant = Color(0xFF1E1E26),
    onBackground = Color(0xFFEAEAF0),
    onSurface = Color(0xFFEAEAF0),
    outline = Color(0xFF2A2A35),
    error = Color(0xFFE05561),
)
