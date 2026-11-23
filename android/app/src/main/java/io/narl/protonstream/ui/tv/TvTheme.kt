package io.narl.protonstream.ui.tv

import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember
import androidx.compose.ui.graphics.luminance
import androidx.tv.material3.ColorScheme
import androidx.tv.material3.LocalContentColor
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.darkColorScheme
import androidx.tv.material3.lightColorScheme
import androidx.compose.material3.ColorScheme as PhoneColorScheme
import androidx.compose.material3.MaterialTheme as PhoneTheme

/**
 * The television's theme: the app's own palette and accent, handed to
 * tv-material, whose cards, buttons and drawer read a colour scheme of their own.
 *
 * Goes inside `ProtonStreamTheme`, which it takes the colours from, so a palette
 * or accent picked in Settings repaints both. A focused card is outlined in the
 * accent (`border`), the cue tv-material draws for focus.
 */
@Composable
internal fun TvTheme(content: @Composable () -> Unit) {
    val phone = PhoneTheme.colorScheme
    val scheme = remember(phone) { tvColors(phone) }
    // tv-material's text is black unless something says otherwise; its own
    // surfaces do, but a page drawn straight on the background has none.
    MaterialTheme(colorScheme = scheme) {
        CompositionLocalProvider(LocalContentColor provides scheme.onBackground, content = content)
    }
}

private fun tvColors(c: PhoneColorScheme): ColorScheme = if (c.background.luminance() > 0.5f) {
    lightColorScheme(
        primary = c.primary,
        onPrimary = c.onPrimary,
        primaryContainer = c.primaryContainer,
        onPrimaryContainer = c.onPrimaryContainer,
        secondary = c.secondary,
        onSecondary = c.onSecondary,
        secondaryContainer = c.secondaryContainer,
        onSecondaryContainer = c.onSecondaryContainer,
        background = c.background,
        onBackground = c.onBackground,
        surface = c.surface,
        onSurface = c.onSurface,
        surfaceVariant = c.surfaceVariant,
        onSurfaceVariant = c.onSurfaceVariant,
        inverseSurface = c.inverseSurface,
        inverseOnSurface = c.inverseOnSurface,
        error = c.error,
        onError = c.onError,
        border = c.primary,
        borderVariant = c.outlineVariant,
        scrim = c.scrim,
    )
} else {
    darkColorScheme(
        primary = c.primary,
        onPrimary = c.onPrimary,
        primaryContainer = c.primaryContainer,
        onPrimaryContainer = c.onPrimaryContainer,
        secondary = c.secondary,
        onSecondary = c.onSecondary,
        secondaryContainer = c.secondaryContainer,
        onSecondaryContainer = c.onSecondaryContainer,
        background = c.background,
        onBackground = c.onBackground,
        surface = c.surface,
        onSurface = c.onSurface,
        surfaceVariant = c.surfaceVariant,
        onSurfaceVariant = c.onSurfaceVariant,
        inverseSurface = c.inverseSurface,
        inverseOnSurface = c.inverseOnSurface,
        error = c.error,
        onError = c.onError,
        border = c.primary,
        borderVariant = c.outlineVariant,
        scrim = c.scrim,
    )
}
