package io.narl.protonstream.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class LanguagePickerTest {
    @Test
    fun `a stored tag reads as its language in either ISO form`() {
        assertEquals("Japanese", languageLabel("jpn"))
        assertEquals("Japanese", languageLabel("ja"))
        assertEquals("German", languageLabel("deu"))
        assertEquals("German", languageLabel("ger"))
    }

    @Test
    fun `a region suffix and capitals do not hide the language`() {
        assertEquals("Portuguese", languageLabel("PT-br"))
    }

    @Test
    fun `a tag not on the list is shown as typed, and blank is no preference`() {
        assertEquals("tgl", languageLabel("tgl"))
        assertEquals("No preference", languageLabel(null))
        assertEquals("No preference", languageLabel("  "))
    }

    @Test
    fun `every listed language has a two-letter form`() {
        LANGUAGES.forEach { (code, name) -> assertEquals(code, name, languageLabel(code)) }
    }
}
