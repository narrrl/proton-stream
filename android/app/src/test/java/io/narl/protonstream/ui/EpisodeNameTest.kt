package io.narl.protonstream.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import uniffi.pstr_android.EpisodeRecord

class EpisodeNameTest {
    private fun episode(name: String, detail: String, providerName: String? = null) = EpisodeRecord(
        shareId = "s", volumeId = "v", linkId = "l", name = name, label = "S01E01", detail = detail,
        season = 1u, number = 1u, size = null, progress = null, resumeAt = null,
        watched = false, offline = false, providerName = providerName, providerOverview = null, stillUrl = null,
        airDate = null, lastPlayed = 0,
    )

    @Test
    fun `the player captions an episode with its name and never its filename`() {
        val file = "[Group] Show - S01E01.mkv"
        assertEquals("Dog & Chainsaw", episode(file, file, providerName = "Dog & Chainsaw").shownName())
        assertEquals("Winter Is Coming", episode(file, "Winter Is Coming").shownName())
        assertNull(episode(file, file).shownName())
        assertNull(episode(file, "").shownName())
    }
}
