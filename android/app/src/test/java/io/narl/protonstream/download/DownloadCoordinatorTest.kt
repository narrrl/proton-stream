package io.narl.protonstream.download

import androidx.work.NetworkType
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.UUID

class DownloadCoordinatorTest {
    @Test
    fun `wifi only downloads require an unmetered network`() {
        assertEquals(NetworkType.UNMETERED, requiredNetworkType(wifiOnly = true))
    }

    @Test
    fun `downloads allow any connected network when wifi only is disabled`() {
        assertEquals(NetworkType.CONNECTED, requiredNetworkType(wifiOnly = false))
    }

    @Test
    fun `foreground notification id is stable and positive per work request`() {
        val workId = UUID.fromString("2f0ba9bc-8936-439b-88fd-cb2741a17653")
        assertEquals(stableNotificationId(workId), stableNotificationId(workId))
        assertTrue(stableNotificationId(workId) > 0)
        val otherWorkId = UUID.fromString("932e1de4-4359-423a-b5e8-af3e43086ab1")
        assertTrue(stableNotificationId(workId) != stableNotificationId(otherWorkId))
    }

    @Test
    fun `share cancellation tags cannot collide with the global queue tag`() {
        assertEquals("offline-share:share-1", DownloadCoordinator.shareTag("share-1"))
        assertTrue(DownloadCoordinator.shareTag("share-1") != DownloadCoordinator.TAG)
    }

    @Test
    fun `episode work names are stable per share and link`() {
        assertEquals(
            "offline-share-1-link-2",
            DownloadCoordinator.workName("share-1", "link-2"),
        )
    }
}
