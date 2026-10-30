package io.narl.protonstream.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.pstr_android.TitleRecord
import uniffi.pstr_android.TitleType

class StatusTest {
    private fun title(status: String?, airing: Boolean = false, next: Pair<UInt, Long>? = null) = TitleRecord(
        key = "k", name = "k", year = null, kind = TitleType.SERIES, watchedCount = 0uL,
        episodeCount = 0uL, canonicalName = null, originalName = null, overview = null,
        metadataProvider = null, metadataId = null, metadataYear = null, metadataKind = null, posterUrl = null,
        backdropUrl = null, rating = null, genres = emptyList(), providerEpisodeCount = null, externalUrl = null,
        manualMatch = false, seasons = emptyList(),
        displayName = "k", wideUrl = null, formatLabel = null, seasonLabel = null, studios = emptyList(),
        directors = emptyList(), tags = emptyList(), airing = airing, nextEpisode = next?.first,
        nextAiringAt = next?.second, franchise = emptyList(), continueIndex = null, status = status,
    )

    @Test
    fun `the status chip names the next episode while one is due`() {
        val (text, live) = statusText(title("Airing", airing = true, next = 8u to 1_800_000_000L))!!
        assertTrue(text, text.startsWith("Airing  ·  episode 8 "))
        assertTrue(live)
    }

    @Test
    fun `a finished title is said plainly and is not live`() {
        assertEquals("Finished" to false, statusText(title("Finished")))
    }

    @Test
    fun `a title with no status and nothing airing has no chip`() {
        assertNull(statusText(title(null)))
        assertEquals("Airing" to true, statusText(title(null, airing = true)))
    }
}
