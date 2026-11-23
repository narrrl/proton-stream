package io.narl.protonstream

import android.media.tv.TvContract.WatchNextPrograms
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.pstr_android.TitleType

class WatchNextTest {
    private val show = titleRecord(
        "show",
        episodeRecord("e1", number = 1u, watched = true, progress = 1.0, lastPlayed = 100),
        episodeRecord(
            "e2", number = 2u, progress = 0.25, resumeAt = 300.0, lastPlayed = 200,
            providerName = "The Second One", stillUrl = "https://image.example/e2.jpg",
        ),
        name = "A Show",
        continueIndex = 1u,
        posterUrl = "https://image.example/poster.jpg",
    )

    @Test
    fun `a part-watched episode is a card to continue, with its place against its length`() {
        val entry = watchNextEntries(listOf(show)).single()
        assertEquals("s/e2", entry.key)
        assertEquals("A Show", entry.title)
        assertEquals("The Second One", entry.episodeTitle)
        assertEquals(1, entry.season)
        assertEquals(2, entry.episode)
        assertTrue(entry.started)
        assertEquals(300_000L, entry.positionMs)
        assertEquals(1_200_000L, entry.durationMs)
        assertEquals(200_000L, entry.engagedAtMs)
        assertEquals("https://image.example/e2.jpg", entry.artUrl)
        assertTrue(entry.wideArt)
    }

    @Test
    fun `the episode after a finished one is a card for what is next, without a place`() {
        val next = titleRecord(
            "show",
            episodeRecord("e1", number = 1u, watched = true, progress = 1.0, resumeAt = null, lastPlayed = 100),
            episodeRecord("e2", number = 2u, runtime = 24u),
            continueIndex = 1u,
            posterUrl = "https://image.example/poster.jpg",
        )
        val entry = watchNextEntries(listOf(next)).single()
        assertFalse(entry.started)
        assertNull(entry.positionMs)
        assertEquals(24 * 60_000L, entry.durationMs)
        // Engaged when the show was last played, not never.
        assertEquals(100_000L, entry.engagedAtMs)
        assertEquals("https://image.example/poster.jpg", entry.artUrl)
        assertFalse(entry.wideArt)
        val columns = watchNextColumns(entry, "intent:#Intent;end")
        assertEquals(WatchNextPrograms.WATCH_NEXT_TYPE_NEXT, columns[WatchNextPrograms.COLUMN_WATCH_NEXT_TYPE])
        assertFalse(columns.containsKey(WatchNextPrograms.COLUMN_LAST_PLAYBACK_POSITION_MILLIS))
    }

    @Test
    fun `a card never carries the file's name`() {
        val unnamed = titleRecord(
            "show",
            episodeRecord("e1", number = 1u, progress = 0.5, lastPlayed = 10),
            continueIndex = 0u,
        )
        val entry = watchNextEntries(listOf(unnamed)).single()
        val shown = watchNextColumns(entry, "intent:#Intent;end").values.filterIsInstance<String>()
        assertTrue(shown.none { it.contains("[Group]") || it.contains(".mkv") })
        assertNull(entry.episodeTitle)
    }

    @Test
    fun `a film is a movie card, with no season or episode`() {
        val film = titleRecord(
            "film",
            episodeRecord("f", number = 1u, progress = 0.5, lastPlayed = 10, stillUrl = "https://image.example/f.jpg"),
            kind = TitleType.FILM,
            continueIndex = 0u,
            backdropUrl = "https://image.example/backdrop.jpg",
        )
        val entry = watchNextEntries(listOf(film)).single()
        val columns = watchNextColumns(entry, "intent:#Intent;end")
        assertEquals(WatchNextPrograms.TYPE_MOVIE, columns[WatchNextPrograms.COLUMN_TYPE])
        assertNull(columns[WatchNextPrograms.COLUMN_SEASON_DISPLAY_NUMBER])
        assertNull(columns[WatchNextPrograms.COLUMN_EPISODE_DISPLAY_NUMBER])
        assertEquals("https://image.example/backdrop.jpg", entry.artUrl)
    }

    private fun entry(key: String, engagedAt: Long) =
        watchNextEntries(listOf(show)).single().copy(key = key, engagedAtMs = engagedAt)

    @Test
    fun `new cards are added, kept ones updated and gone ones removed`() {
        val rows = listOf(
            WatchNextRow(1, "kept", browsable = true, engagedAtMs = 5),
            WatchNextRow(2, "gone", browsable = true, engagedAtMs = 5),
        )
        val kept = entry("kept", 10)
        val new = entry("new", 20)
        assertEquals(
            listOf(
                WatchNextChange.Update(1, kept),
                WatchNextChange.Delete(2),
                WatchNextChange.Insert(new),
            ),
            planWatchNext(rows, listOf(kept, new)),
        )
    }

    @Test
    fun `a card the viewer removed stays off the home screen until they watch it again`() {
        val removed = listOf(WatchNextRow(7, "s/e2", browsable = false, engagedAtMs = 200_000))
        assertEquals(emptyList<WatchNextChange>(), planWatchNext(removed, listOf(entry("s/e2", 200_000))))
        val watchedAgain = entry("s/e2", 300_000)
        assertEquals(
            listOf(WatchNextChange.Delete(7), WatchNextChange.Insert(watchedAgain)),
            planWatchNext(removed, listOf(watchedAgain)),
        )
    }

    @Test
    fun `a second row for the same episode is removed`() {
        val rows = listOf(
            WatchNextRow(1, "a", browsable = true, engagedAtMs = 0),
            WatchNextRow(2, "a", browsable = true, engagedAtMs = 0),
            WatchNextRow(3, null, browsable = true, engagedAtMs = 0),
        )
        val a = entry("a", 1)
        assertEquals(
            listOf(WatchNextChange.Update(1, a), WatchNextChange.Delete(2), WatchNextChange.Delete(3)),
            planWatchNext(rows, listOf(a)),
        )
    }
}
