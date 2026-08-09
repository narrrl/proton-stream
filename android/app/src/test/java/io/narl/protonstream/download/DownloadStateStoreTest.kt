package io.narl.protonstream.download

import android.content.Context
import androidx.test.core.app.ApplicationProvider
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

@RunWith(RobolectricTestRunner::class)
class DownloadStateStoreTest {
    private lateinit var context: Context
    private lateinit var store: DownloadStateStore

    @Before
    fun setUp() {
        context = ApplicationProvider.getApplicationContext()
        context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE).edit().clear().commit()
        store = DownloadStateStore(context)
    }

    private fun record(
        shareId: String = "share-1",
        linkId: String = "link-1",
        label: String = "S01E01.mkv",
        status: String = RetainedDownload.STATUS_RUNNING,
        error: String? = null,
    ) = RetainedDownload(
        shareId = shareId,
        volumeId = "volume-1",
        linkId = linkId,
        label = label,
        downloaded = 4L * 1024 * 1024,
        total = 761L * 1024 * 1024,
        status = status,
        error = error,
    )

    @Test
    fun `a record survives a round trip unchanged`() {
        val original = record()
        store.put(original)
        assertEquals(original, store.get("share-1", "link-1"))
    }

    @Test
    fun `an absent record reads as null rather than a default`() {
        assertNull(store.get("share-1", "link-1"))
    }

    @Test
    fun `an error message round trips and a null one stays null`() {
        store.put(record(error = "no space left on device"))
        assertEquals("no space left on device", store.get("share-1", "link-1")?.error)

        store.put(record(error = null))
        assertNull(store.get("share-1", "link-1")?.error)
    }

    /**
     * `encode` writes `error` through `JSONObject.put`, which *removes* the key
     * when the value is null. `decode` then reads `optString`, which answers ""
     * rather than null. The blank check is the only thing standing between a
     * cleared error and a download that renders as permanently failed.
     */
    @Test
    fun `a blank error is not read as a failure message`() {
        writeRaw(
            "share-1",
            "link-1",
            baseJson().put("error", ""),
        )
        assertNull(store.get("share-1", "link-1")?.error)
    }

    /** Rows written before `label` existed fall back to the link id. */
    @Test
    fun `a record with no label is captioned with its link id`() {
        writeRaw("share-1", "link-1", baseJson().apply { remove("label") })
        assertEquals("link-1", store.get("share-1", "link-1")?.label)
    }

    @Test
    fun `a corrupt row is skipped rather than failing the whole list`() {
        store.put(record(linkId = "link-1", label = "good.mkv"))
        context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
            .edit()
            .putString("share-1\u001flink-broken", "{ not json")
            .commit()

        val records = store.records()
        assertEquals(1, records.size)
        assertEquals("good.mkv", records.first().label)
    }

    @Test
    fun `records are ordered by label without regard to case`() {
        store.put(record(linkId = "a", label = "beta.mkv"))
        store.put(record(linkId = "b", label = "Alpha.mkv"))
        assertEquals(listOf("Alpha.mkv", "beta.mkv"), store.records().map { it.label })
    }

    @Test
    fun `removing one share leaves the others alone`() {
        store.put(record(shareId = "share-1", linkId = "a"))
        store.put(record(shareId = "share-2", linkId = "b"))

        store.removeShare("share-1")

        assertEquals(listOf("share-2"), store.records().map { it.shareId })
    }

    @Test
    fun `clear forgets everything`() {
        store.put(record(linkId = "a"))
        store.put(record(linkId = "b"))
        store.clear()
        assertTrue(store.records().isEmpty())
    }

    /**
     * The key is what `DownloadCoordinator` and `OfflineDownloadWorker` agree on
     * across processes; two shares may legitimately hold the same link id.
     */
    @Test
    fun `the key separates share from link so ids cannot run together`() {
        store.put(record(shareId = "a", linkId = "b-c"))
        store.put(record(shareId = "a-b", linkId = "c"))
        assertEquals(2, store.records().size)
    }

    private fun baseJson() = JSONObject()
        .put("share", "share-1")
        .put("volume", "volume-1")
        .put("link", "link-1")
        .put("label", "S01E01.mkv")
        .put("downloaded", 0)
        .put("total", 0)
        .put("status", RetainedDownload.STATUS_QUEUED)

    private fun writeRaw(shareId: String, linkId: String, json: JSONObject) {
        context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
            .edit()
            .putString("$shareId\u001f$linkId", json.toString())
            .commit()
    }

    private companion object {
        const val PREFERENCES = "offline_download_state"
    }
}
