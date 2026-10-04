package io.narl.protonstream.ui

import java.time.LocalDate
import java.util.Locale
import org.junit.Assert.assertEquals
import org.junit.Test
import uniffi.pstr_android.EpisodeRecord
import uniffi.pstr_android.SeasonRecord
import uniffi.pstr_android.TitleRecord
import uniffi.pstr_android.TitleType

class HistoryTest {
    private fun episode(link: String, watched: Boolean, progress: Double?, lastPlayed: Long) = EpisodeRecord(
        shareId = "s", volumeId = "v", linkId = link, name = "$link.mkv", label = link, detail = "$link.mkv",
        season = 1u, number = null, size = null, progress = progress, resumeAt = progress?.let { it * 1000 },
        watched = watched, offline = false, providerName = null, providerOverview = null, stillUrl = null,
        airDate = null, lastPlayed = lastPlayed,
    )

    private fun title(key: String, vararg episodes: EpisodeRecord) = TitleRecord(
        key = key, name = key, year = null, kind = TitleType.SERIES, watchedCount = 0uL,
        episodeCount = episodes.size.toULong(), canonicalName = null, originalName = null, overview = null,
        metadataProvider = null, metadataId = null, metadataYear = null, metadataKind = null, posterUrl = null,
        backdropUrl = null, rating = null, genres = emptyList(), providerEpisodeCount = null, externalUrl = null,
        manualMatch = false, seasons = listOf(SeasonRecord(1u, "Season 1", episodes.toList())),
        displayName = key, wideUrl = null, formatLabel = null, seasonLabel = null, studios = emptyList(),
        directors = emptyList(), tags = emptyList(), airing = false, nextEpisode = null, nextAiringAt = null,
        franchise = emptyList(),
    )

    @Test
    fun `history is every started or finished episode, newest first, across titles`() {
        val titles = listOf(
            title(
                "a",
                episode("a1", watched = true, progress = 1.0, lastPlayed = 30),
                // Marked unwatched again: a row at zero, which is not a play.
                episode("a2", watched = false, progress = 0.0, lastPlayed = 40),
                episode("a3", watched = false, progress = null, lastPlayed = 0),
            ),
            title("b", episode("b1", watched = false, progress = 0.4, lastPlayed = 50)),
        )
        val played = history(titles)
        assertEquals(listOf("b1", "a1"), played.map { it.episode.linkId })
        assertEquals(0, played.last().index)
    }

    @Test
    fun `days are headed relative to today, then by date`() {
        val today = LocalDate.of(2026, 10, 3) // a Saturday
        val english = Locale.ENGLISH
        assertEquals("Today", dayHeading(today, today, english))
        assertEquals("Yesterday", dayHeading(today.minusDays(1), today, english))
        assertEquals("Monday", dayHeading(LocalDate.of(2026, 9, 28), today, english))
        assertEquals("26 September", dayHeading(LocalDate.of(2026, 9, 26), today, english))
        assertEquals("3 October 2025", dayHeading(LocalDate.of(2025, 10, 3), today, english))
    }
}
