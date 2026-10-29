package io.narl.protonstream.ui

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class LogoAndRuntimeTest {
    @Test
    fun `a padded logo is bounded by its lettering and not its haze`() {
        val width = 10
        val pixels = IntArray(width * 6)
        // Lettering from (2, 1) to (6, 3), and a pixel of haze in the corner.
        for (y in 1..3) for (x in 2..6) pixels[y * width + x] = 0xFFFFFFFF.toInt()
        pixels[5 * width + 9] = 0x03FFFFFF
        assertArrayEquals(intArrayOf(2, 1, 6, 3), opaqueBounds(pixels, width))
    }

    @Test
    fun `a logo with nothing visible has no bounds`() {
        assertNull(opaqueBounds(IntArray(16), 4))
    }

    @Test
    fun `a running time reads in hours and minutes`() {
        assertEquals("24 min", formatRuntime(24u))
        assertEquals("2 h", formatRuntime(120u))
        assertEquals("2 h 4 min", formatRuntime(124u))
    }
}
