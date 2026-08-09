package io.narl.protonstream.ui.theme

import androidx.compose.material3.Typography
import androidx.compose.ui.text.TextStyle
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * That the type ramp is the ramp, and the corner radius is the radius.
 *
 * The two clients share a palette because one Rust function resolves it for
 * both. Type and shape have no such bridge — they are named in
 * `pstr_app::theme::Role` and copied here — so what stops them drifting is this
 * test and the numbers in it. Change the Rust ramp and this fails; that is the
 * point of it.
 *
 * Every style is named rather than checked against a set, because "some rung"
 * is not the property that matters: `bodyMedium`'s Material default is 14sp,
 * which is a rung, so a `bodyMedium` nobody mapped would pass a looser check
 * while sitting one step off where it belongs.
 */
class TypeRampTest {
    /** `Role::size()`, in the order `theme.rs` declares it. */
    private val ramp = listOf(26f, 20f, 18f, 17f, 15f, 14f, 13f, 12f, 11f)

    private val expected = mapOf(
        "displayLarge" to 26f,
        "displayMedium" to 26f,
        "displaySmall" to 20f,
        "headlineLarge" to 26f,
        "headlineMedium" to 26f,
        "headlineSmall" to 18f,
        "titleLarge" to 20f,
        "titleMedium" to 17f,
        "titleSmall" to 15f,
        "bodyLarge" to 14f,
        "bodyMedium" to 13f,
        "bodySmall" to 12f,
        "labelLarge" to 13f,
        "labelMedium" to 12f,
        "labelSmall" to 11f,
    )

    @Test
    fun `every text style sits on the shared ramp`() {
        expected.forEach { (name, size) ->
            assertEquals(name, size, styles().getValue(name).fontSize.value, 0.001f)
        }
    }

    /**
     * Material adds type styles between versions, and one added tomorrow would
     * arrive at whatever size Material chose for it. The list above is only as
     * complete as the day it was written unless something says so.
     */
    @Test
    fun `no text style is left at Material's own size`() {
        assertEquals(expected.keys, styles().keys)
    }

    /**
     * Leading is derived from the size, so the only thing worth asserting is
     * that it *was* derived — a style that kept Material's line height carries
     * a number with no relation to its own size.
     */
    @Test
    fun `line height follows the size it belongs to`() {
        styles().forEach { (name, style) ->
            assertEquals(name, style.fontSize.value * 1.35f, style.lineHeight.value, 0.01f)
        }
    }

    @Test
    fun `the ramp has no rung the styles never reach`() {
        val drawn = styles().values.map { it.fontSize.value }.toSet()
        ramp.forEach { rung -> assertTrue("$rung is named but never drawn", rung in drawn) }
    }

    @Test
    fun `every surface is drawn at the desktop's radius`() {
        listOf(ProtonShapes.small, ProtonShapes.medium, ProtonShapes.large).forEach { shape ->
            // `CornerSize` resolves against a size and a density, and neither is
            // available to a unit test; `toString` is the only handle on the
            // value that does not need a composition.
            assertTrue("$shape is not radius $CORNER", shape.toString().contains("$CORNER"))
        }
    }

    /**
     * Every text style a caller can set, read by name, whatever the version
     * names.
     *
     * Material 1.4 carries a second, `Emphasized` copy of all fifteen for its
     * expressive components. Those are `internal` — the constructor takes them
     * but Kotlin will not let this module pass them — so they keep Material's
     * own sizes and nothing here can do anything about it. Their getters are
     * name-mangled with `$material3`, which is how they are told apart from a
     * public style that genuinely went unmapped.
     */
    private fun styles(): Map<String, TextStyle> = Typography::class.java.declaredMethods
        .filter { it.parameterCount == 0 && it.returnType == TextStyle::class.java }
        .filterNot { '$' in it.name }
        .associate { method ->
            method.isAccessible = true
            method.name.removePrefix("get").replaceFirstChar(Char::lowercaseChar) to
                method.invoke(ProtonTypography) as TextStyle
        }
}
