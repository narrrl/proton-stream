package io.narl.protonstream.ui.theme

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

private val ProtonColors = darkColorScheme(
    primary = Color(0xFFCBA6F7),
    onPrimary = Color(0xFF1E1E2E),
    secondary = Color(0xFFF5C2E7),
    background = Color(0xFF1E1E2E),
    surface = Color(0xFF181825),
    surfaceVariant = Color(0xFF313244),
    onBackground = Color(0xFFCDD6F4),
    onSurface = Color(0xFFCDD6F4),
    outline = Color(0xFF6C7086),
    error = Color(0xFFF38BA8),
)

@Composable
fun ProtonStreamTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = ProtonColors, content = content)
}
