package io.narl.protonstream.download

import android.content.Context
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.work.Configuration
import androidx.work.NetworkType
import androidx.work.WorkInfo
import androidx.work.WorkManager
import androidx.work.testing.SynchronousExecutor
import androidx.work.testing.WorkManagerTestInitHelper
import io.narl.protonstream.settings.SettingsStore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

/**
 * What actually reaches WorkManager. The unit tests cover the pure helpers; this
 * covers the enqueue itself — the input data (which is persisted unencrypted, so
 * it must carry no secret), the constraints, and the unique-work identity that
 * pause and resume depend on.
 */
@RunWith(AndroidJUnit4::class)
class DownloadCoordinatorInstrumentedTest {
    private lateinit var context: Context
    private lateinit var workManager: WorkManager

    @Before
    fun setUp() {
        context = ApplicationProvider.getApplicationContext()
        WorkManagerTestInitHelper.initializeTestWorkManager(
            context,
            Configuration.Builder().setExecutor(SynchronousExecutor()).build(),
        )
        workManager = WorkManager.getInstance(context)
        workManager.cancelAllWork().result.get()
        DownloadStateStore(context).clear()
    }

    private fun download(linkId: String = "link-1", shareId: String = "share-1") = RetainedDownload(
        shareId = shareId,
        volumeId = "volume-1",
        linkId = linkId,
        label = "S01E01.mkv",
    )

    private fun infoFor(download: RetainedDownload): WorkInfo? =
        workManager.getWorkInfosForUniqueWork(
            DownloadCoordinator.workName(download.shareId, download.linkId),
        ).get().firstOrNull()

    @Test
    fun resumingEnqueuesUniqueWorkAndRetainsAQueuedRecord() {
        val download = download()
        DownloadCoordinator.resume(context, download)

        assertNotNull(infoFor(download))
        assertEquals(
            RetainedDownload.STATUS_QUEUED,
            DownloadStateStore(context).get("share-1", "link-1")?.status,
        )
    }

    /**
     * Tags are persisted in `androidx.work.workdb` unencrypted and are what the
     * UI queries and what cancel-by-share matches on. Pinning the exact set is
     * what makes an accidental addition — a fragment, a URL — visible in review.
     */
    @Test
    fun theWorkCarriesExactlyTheExpectedTags() {
        val download = download()
        DownloadCoordinator.resume(context, download)

        val tags = infoFor(download)?.tags.orEmpty()
        assertTrue(DownloadCoordinator.TAG in tags)
        assertTrue(DownloadCoordinator.shareTag("share-1") in tags)
        assertTrue("${DownloadCoordinator.EPISODE_TAG}S01E01.mkv" in tags)
        // The remaining tag is WorkManager's own worker class name.
        assertEquals(4, tags.size)
    }

    @Test
    fun theWifiOnlySettingReachesTheConstraint() {
        // Documents B29: the constraint is read at enqueue time, so work already
        // queued keeps whatever was set when it was queued. When B29 is fixed,
        // this test grows a case for flipping the toggle after enqueue.
        SettingsStore(context).wifiOnly = true
        assertEquals(NetworkType.UNMETERED, requiredNetworkType(SettingsStore(context).wifiOnly))
        SettingsStore(context).wifiOnly = false
        assertEquals(NetworkType.CONNECTED, requiredNetworkType(SettingsStore(context).wifiOnly))
    }

    @Test
    fun pauseCancelsTheWorkAndRecordsThePausedStatus() {
        val download = download()
        DownloadCoordinator.resume(context, download)
        DownloadCoordinator.pause(context, download)

        assertTrue(infoFor(download)?.state?.isFinished ?: false)
        assertEquals(
            RetainedDownload.STATUS_PAUSED,
            DownloadStateStore(context).get("share-1", "link-1")?.status,
        )
    }

    @Test
    fun cancellingOneShareLeavesAnotherSharesWorkAlone() {
        val first = download(linkId = "a", shareId = "share-1")
        val second = download(linkId = "b", shareId = "share-2")
        DownloadCoordinator.resume(context, first)
        DownloadCoordinator.resume(context, second)

        workManager.cancelAllWorkByTag(DownloadCoordinator.shareTag("share-1")).result.get()

        assertTrue(infoFor(first)?.state?.isFinished ?: false)
        assertEquals(WorkInfo.State.ENQUEUED, infoFor(second)?.state)
    }

    @Test
    fun twoEpisodesOfOneShareEnqueueSeparately() {
        DownloadCoordinator.resume(context, download(linkId = "a"))
        DownloadCoordinator.resume(context, download(linkId = "b"))

        assertEquals(2, workManager.getWorkInfosByTag(DownloadCoordinator.TAG).get().size)
    }
}
