package io.narl.protonstream.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class ArtworkTest {
    @Test
    fun `initials skip the lower-case particles of a romanised name`() {
        assertEquals("OK", initials("Oshi no Ko"))
        assertEquals("AH", initials("Ano Hi Mita Hana no Namae o Bokutachi wa Mada Shiranai."))
    }

    @Test
    fun `a one-word name has one initial`() {
        assertEquals("A", initials("Akira"))
    }

    @Test
    fun `a name with no capitals falls back to its first two words`() {
        assertEquals("BW", initials("blue world order"))
    }

    @Test
    fun `punctuation and separators are not words`() {
        assertEquals("S2", initials("Steins;Gate - 2"))
        assertEquals("", initials(" - "))
    }
}
