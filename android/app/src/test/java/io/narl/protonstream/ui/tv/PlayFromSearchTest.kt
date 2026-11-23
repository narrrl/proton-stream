package io.narl.protonstream.ui.tv

import io.narl.protonstream.episodeRecord
import io.narl.protonstream.titleRecord
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PlayFromSearchTest {
    private val steins = titleRecord(
        "steins",
        episodeRecord("s1", number = 1u, watched = true, progress = 1.0, lastPlayed = 10),
        episodeRecord("s2", number = 2u),
        name = "Steins;Gate",
        continueIndex = 1u,
    )
    private val frieren = titleRecord(
        "frieren",
        episodeRecord("f1", number = 1u, progress = 0.5, lastPlayed = 50),
        name = "Frieren",
        continueIndex = 0u,
    )

    @Test
    fun `a title is found by what was heard, without case or punctuation`() {
        assertEquals(steins to 1, playFromSearch(listOf(steins, frieren), "steins gate"))
    }

    @Test
    fun `nothing or two titles of that name are a search, not a guess`() {
        assertNull(playFromSearch(listOf(steins, frieren), "gate"))
        val again = titleRecord("steins-2", episodeRecord("x"), name = "Steins Gate")
        assertNull(playFromSearch(listOf(steins, again), "steins gate"))
    }

    @Test
    fun `a blank query plays the top of continue watching`() {
        assertEquals(frieren to 0, playFromSearch(listOf(steins, frieren), ""))
    }
}
