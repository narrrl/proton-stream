package io.narl.protonstream.download

import android.content.Context
import androidx.work.Constraints
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.workDataOf
import io.narl.protonstream.settings.SettingsStore
import uniffi.pstr_android.EpisodeRecord

object DownloadCoordinator {
    fun enqueue(context: Context, episode: EpisodeRecord) {
        enqueue(
            context,
            RetainedDownload(episode.shareId, episode.volumeId, episode.linkId, episode.label),
            ExistingWorkPolicy.KEEP,
        )
    }

    fun resume(context: Context, download: RetainedDownload) {
        enqueue(context, download, ExistingWorkPolicy.REPLACE)
    }

    private fun enqueue(context: Context, download: RetainedDownload, policy: ExistingWorkPolicy) {
        DownloadStateStore(context).put(
            download.copy(status = RetainedDownload.STATUS_QUEUED, error = null),
        )
        val request = OneTimeWorkRequestBuilder<OfflineDownloadWorker>()
            .setConstraints(
                Constraints.Builder().setRequiredNetworkType(
                    requiredNetworkType(SettingsStore(context).wifiOnly),
                ).build(),
            )
            .setInputData(
                workDataOf(
                    OfflineDownloadWorker.KEY_SHARE_ID to download.shareId,
                    OfflineDownloadWorker.KEY_VOLUME_ID to download.volumeId,
                    OfflineDownloadWorker.KEY_LINK_ID to download.linkId,
                    OfflineDownloadWorker.KEY_LABEL to download.label,
                ),
            )
            .addTag(TAG)
            .addTag("$EPISODE_TAG${download.label}")
            .addTag(shareTag(download.shareId))
            .build()
        WorkManager.getInstance(context).enqueueUniqueWork(
            workName(download.shareId, download.linkId),
            policy,
            request,
        )
    }

    fun enqueue(context: Context, episodes: Iterable<EpisodeRecord>) {
        episodes.filterNot(EpisodeRecord::offline).forEach { enqueue(context, it) }
    }

    fun pause(context: Context, download: RetainedDownload) {
        DownloadStateStore(context).put(download.copy(status = RetainedDownload.STATUS_PAUSED, error = null))
        WorkManager.getInstance(context).cancelUniqueWork(workName(download.shareId, download.linkId))
    }

    fun cancel(context: Context, download: RetainedDownload) {
        DownloadStateStore(context).put(download.copy(status = RetainedDownload.STATUS_CANCELLED))
        WorkManager.getInstance(context).cancelUniqueWork(workName(download.shareId, download.linkId))
    }

    fun shareTag(shareId: String) = "$SHARE_TAG$shareId"
    fun workName(shareId: String, linkId: String) = "offline-$shareId-$linkId"

    const val TAG = "offline-download"
    const val EPISODE_TAG = "offline-episode:"
    private const val SHARE_TAG = "offline-share:"
}

internal fun requiredNetworkType(wifiOnly: Boolean): NetworkType =
    if (wifiOnly) NetworkType.UNMETERED else NetworkType.CONNECTED
