package io.narl.protonstream.ui.theme

import androidx.compose.material3.ColorScheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.ui.graphics.Color
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.pstr_android.PaletteRecord

/**
 * That every Material colour role is answered by the palette.
 *
 * Material names roughly fifty roles and a `ColorScheme` is built by copying a
 * baseline, so a role nobody names keeps Material's own purple-and-grey —
 * silently, and only where that particular role happens to be drawn. That is
 * how `surfaceContainerHighest` went unnoticed: nothing referred to it by name,
 * but it is what `Card` fills with, so every tile, episode row, share and
 * download in the app was baseline neutral on a flavoured page.
 *
 * The check is reflective rather than a list, because a list is exactly as
 * incomplete as the mapping it was written from — and because Material adds
 * roles between versions, and a role added tomorrow should fail this the day
 * the dependency moves.
 */
class SchemeRolesTest {
    /**
     * A palette whose every colour is distinct and none of which Material would
     * ever produce, so that "this role came from the palette" is decidable by
     * looking at the value.
     */
    private fun sentinels(light: Boolean) = PaletteRecord(
        background = 0xFF010203u,
        surface = 0xFF040506u,
        sunken = 0xFF070809u,
        card = 0xFF0A0B0Cu,
        cardHover = 0xFF0D0E0Fu,
        border = 0xFF101112u,
        text = 0xFF131415u,
        muted = 0xFF161718u,
        accent = 0xFF191A1Bu,
        accentAlt = 0xFF1C1D1Eu,
        accentDim = 0xFF1F2021u,
        onAccent = 0xFF222324u,
        danger = 0xFF252627u,
        elevated = 0xFF28292Au,
        dangerDim = 0xFF2B2C2Du,
        light = light,
    )

    /** Every role on a `ColorScheme`, read by name, whatever the version names. */
    private fun roles(scheme: ColorScheme): Map<String, Color> =
        ColorScheme::class.java.declaredMethods
            .filter { it.name.startsWith("get") && it.parameterCount == 0 && it.returnType == Long::class.java }
            .associate { method ->
                method.isAccessible = true
                // Kotlin mangles the getter of a value-class property:
                // `getPrimary-0d7_KjU`. The property is what the name says up
                // to the dash.
                method.name.removePrefix("get").substringBefore('-') to
                    Color((method.invoke(scheme) as Long).toULong())
            }

    @Test
    fun `every role is answered by the palette`() {
        for (light in listOf(false, true)) {
            val palette = sentinels(light)
            val allowed = buildSet {
                addAll(
                    listOf(
                        palette.background, palette.surface, palette.sunken, palette.card,
                        palette.cardHover, palette.border, palette.text, palette.muted,
                        palette.accent, palette.accentAlt, palette.accentDim, palette.onAccent,
                        palette.danger, palette.elevated, palette.dangerDim,
                    ).map { Color(it.toInt()) },
                )
                // The two roles that are deliberately not a palette colour: no
                // elevation tint at all, and a scrim that is an absence of
                // light rather than a hue.
                add(Color.Transparent)
                add(Color.Black)
            }

            val leaked = roles(schemeOf(palette)).filterValues { it !in allowed }
            assertTrue(
                "light=$light: ${leaked.keys.sorted()} still carry Material's baseline",
                leaked.isEmpty(),
            )
        }
    }

    /**
     * The count is asserted too. A reflective sweep that found no roles at all
     * would pass the check above for the wrong reason.
     */
    @Test
    fun `the sweep sees the whole scheme`() {
        val count = roles(darkColorScheme()).size
        assertTrue("only $count roles found; the reflective sweep is not working", count >= 40)
        assertEquals(count, roles(lightColorScheme()).size)
    }

    /**
     * Which baseline was copied stops being observable once every role is
     * mapped — and that is the point.
     *
     * `light` still decides plenty, but it decides it in Rust, where it picks
     * the flavour's colours. By the time a palette reaches Kotlin the
     * light-or-dark question is already answered *inside* the colours, so two
     * palettes that differ only in the flag have to produce the same scheme. If
     * they ever stop doing so, something is reading `light` here rather than
     * reading the colours.
     */
    @Test
    fun `the baseline no longer shows through`() {
        assertEquals(roles(schemeOf(sentinels(false))), roles(schemeOf(sentinels(true))))
        assertEquals(Color.Transparent, schemeOf(sentinels(false)).surfaceTint)
    }

    /** The fallback goes through the same mapping as any other palette. */
    @Test
    fun `the shipped fallback is fully mapped too`() {
        val scheme = schemeOf(ProtonFallback)
        assertEquals(Color(ProtonFallback.elevated.toInt()), scheme.surfaceContainerHighest)
        assertEquals(Color(ProtonFallback.card.toInt()), scheme.surfaceContainer)
        assertEquals(Color(ProtonFallback.dangerDim.toInt()), scheme.errorContainer)
    }
}
