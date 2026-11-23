package io.narl.protonstream

import android.app.SearchManager
import android.content.Intent
import android.provider.MediaStore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class TvRequestTest {
    private fun request(action: String?, vararg extras: Pair<String, String>) =
        tvRequestFrom(action) { name -> extras.toMap()[name] }

    @Test
    fun `a watch next card asks for its episode of its title`() {
        assertEquals(
            TvRequest.Play("title", "link"),
            request(ACTION_PLAY_EPISODE, EXTRA_TITLE_KEY to "title", EXTRA_LINK_ID to "link"),
        )
        assertNull(request(ACTION_PLAY_EPISODE, EXTRA_TITLE_KEY to "title"))
    }

    @Test
    fun `a spoken search is a search, from the platform or the assistant`() {
        assertEquals(TvRequest.Search("frieren"), request(Intent.ACTION_SEARCH, SearchManager.QUERY to " frieren "))
        assertEquals(TvRequest.Search("frieren"), request(ACTION_ASSISTANT_SEARCH, SearchManager.QUERY to "frieren"))
        assertNull(request(Intent.ACTION_SEARCH, SearchManager.QUERY to "  "))
    }

    @Test
    fun `play from search keeps a blank query, which asks for anything`() {
        assertEquals(TvRequest.PlayFromSearch(""), request(MediaStore.INTENT_ACTION_MEDIA_PLAY_FROM_SEARCH))
        assertEquals(
            TvRequest.PlayFromSearch("frieren"),
            request(MediaStore.INTENT_ACTION_MEDIA_PLAY_FROM_SEARCH, SearchManager.QUERY to "frieren"),
        )
    }

    @Test
    fun `other intents ask for nothing`() {
        assertNull(request(Intent.ACTION_MAIN))
        assertNull(request(null))
    }
}
