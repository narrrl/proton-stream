package io.narl.protonstream

import android.content.Intent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class IncomingShareLinkTest {
    private val link = "https://drive.proton.me/urls/ABC123#s3cr3t"

    @Test
    fun `a viewed share link is taken whole, fragment included`() {
        assertEquals(link, shareLinkFrom(Intent.ACTION_VIEW, link, null))
    }

    @Test
    fun `a viewed url that is not an https share link is ignored`() {
        assertNull(shareLinkFrom(Intent.ACTION_VIEW, "https://drive.proton.me/u/0/", null))
        assertNull(shareLinkFrom(Intent.ACTION_VIEW, "http://drive.proton.me/urls/ABC123#s3cr3t", null))
    }

    @Test
    fun `a shared message yields the link inside it`() {
        assertEquals(link, shareLinkFrom(Intent.ACTION_SEND, null, "Season two is up: $link enjoy"))
        assertEquals(link, shareLinkFrom(Intent.ACTION_SEND, null, "(<$link>)"))
    }

    @Test
    fun `a shared message without a link yields nothing`() {
        assertNull(shareLinkFrom(Intent.ACTION_SEND, null, "no link here"))
    }

    @Test
    fun `other actions are ignored`() {
        assertNull(shareLinkFrom(Intent.ACTION_MAIN, link, link))
    }
}
