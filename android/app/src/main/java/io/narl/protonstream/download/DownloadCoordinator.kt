package io.narl.protonstream.download

import android.content.Context
import android.os.StatFs
import androidx.work.Constraints
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequest
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.workDataOf
import io.narl.protonstream.settings.SettingsStore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import uniffi.pstr_android.EpisodeRecord

object DownloadCoordinator {
    /**
     * Where bulk queueing runs.
     *
     * Callers are click handlers, and "Download show" on a long series is a
     * WorkManager enqueue per episode — each of which writes to its own
     * database. On the main thread that is an ANR waiting for a long enough
     * series.
     */
    private val queueing = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    fun enqueue(context: Context, episode: EpisodeRecord) = enqueue(context, listOf(episode))

    fun resume(context: Context, download: RetainedDownload) {
        enqueue(context, download, ExistingWorkPolicy.REPLACE)
    }

    private fun enqueue(context: Context, download: RetainedDownload, policy: ExistingWorkPolicy) {
        val queued = download.copy(status = RetainedDownload.STATUS_QUEUED, error = null)
        DownloadStateStore(context).put(queued)
        WorkManager.getInstance(context).enqueueUniqueWork(
            workName(download.shareId, download.linkId),
            policy,
            request(queued, SettingsStore(context).wifiOnly),
        )
    }

    fun enqueue(context: Context, episodes: Iterable<EpisodeRecord>) {
        val wanted = episodes.filterNot(EpisodeRecord::offline).toList()
        if (wanted.isEmpty()) return
        val application = context.applicationContext
        queueing.launch {
            val wifiOnly = SettingsStore(application).wifiOnly
            val store = DownloadStateStore(application)
            val manager = WorkManager.getInstance(application)
            // Space is checked once for the whole request, against its total.
            // Queueing a season that cannot fit and discovering it episode by
            // episode at ENOSPC is how the app fills its own database's disk.
            val room = roomFor(application, wanted.sumOf { it.size?.toLong() ?: 0L })
            val downloads = wanted.map { episode ->
                RetainedDownload(
                    episode.shareId,
                    episode.volumeId,
                    episode.linkId,
                    episode.label,
                    total = episode.size?.toLong() ?: 0L,
                    status = if (room) RetainedDownload.STATUS_QUEUED else RetainedDownload.STATUS_FAILED,
                    error = if (room) null else "Not enough free space",
                )
            }
            store.putAll(downloads)
            if (!room) return@launch
            downloads.forEach { download ->
                manager.enqueueUniqueWork(
                    workName(download.shareId, download.linkId),
                    ExistingWorkPolicy.KEEP,
                    request(download, wifiOnly),
                )
            }
        }
    }

    /**
     * Whether [bytes] plus working room will fit where offline files are kept.
     *
     * `setRequiresStorageNotLow` only holds work back at Android's own low-storage
     * threshold, which a single episode can cross on its own. An unknown size —
     * zero — is allowed through: refusing a download because the catalog has no
     * size for it would be worse than the ENOSPC it is guarding against.
     */
    private fun roomFor(context: Context, bytes: Long): Boolean {
        if (bytes <= 0) return true
        val available = runCatching { StatFs(context.filesDir.path).availableBytes }.getOrNull() ?: return true
        return available - bytes >= STORAGE_HEADROOM
    }

    /**
     * Re-apply the network constraint to work that is already queued.
     *
     * Constraints are baked in at enqueue time, so turning "Wi-Fi only" on after
     * queueing a season did nothing to that season — it downloaded over cellular
     * on the way out of the house, which is the one moment the setting exists
     * for. Running work is left alone: WorkManager stops it on its own when the
     * new constraint stops being met.
     */
    fun applyNetworkPolicy(context: Context) {
        val application = context.applicationContext
        queueing.launch {
            val wifiOnly = SettingsStore(application).wifiOnly
            val manager = WorkManager.getInstance(application)
            DownloadStateStore(application).records()
                .filter { it.status == RetainedDownload.STATUS_QUEUED || it.status == RetainedDownload.STATUS_RUNNING }
                .forEach { download ->
                    manager.enqueueUniqueWork(
                        workName(download.shareId, download.linkId),
                        ExistingWorkPolicy.REPLACE,
                        request(download, wifiOnly),
                    )
                }
        }
    }

    fun pause(context: Context, download: RetainedDownload) =
        setStatus(context, download, RetainedDownload.STATUS_PAUSED)

    fun cancel(context: Context, download: RetainedDownload) =
        setStatus(context, download, RetainedDownload.STATUS_CANCELLED)

    /**
     * The status write goes before the work is cancelled, and through `update`
     * rather than `put`: the running worker is writing progress into the same
     * record, and a whole-record `put` from a screen's stale copy would both
     * lose that progress and race the worker's own read-modify-write.
     */
    private fun setStatus(context: Context, download: RetainedDownload, status: String) {
        DownloadStateStore(context).update(download.shareId, download.linkId) { stored ->
            (stored ?: download).copy(status = status, error = null)
        }
        WorkManager.getInstance(context).cancelUniqueWork(workName(download.shareId, download.linkId))
    }

    private fun request(download: RetainedDownload, wifiOnly: Boolean): OneTimeWorkRequest =
        OneTimeWorkRequestBuilder<OfflineDownloadWorker>()
            .setConstraints(
                Constraints.Builder()
                    .setRequiredNetworkType(requiredNetworkType(wifiOnly))
                    // A download that fills the disk takes the app's own
                    // database down with it; the `.part` file makes waiting free.
                    .setRequiresStorageNotLow(true)
                    .build(),
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

    fun shareTag(shareId: String) = "$SHARE_TAG$shareId"
    fun workName(shareId: String, linkId: String) = "offline-$shareId-$linkId"

    const val TAG = "offline-download"
    const val EPISODE_TAG = "offline-episode:"
    private const val SHARE_TAG = "offline-share:"

    /** Free space to leave behind a download, for the catalog and the caches. */
    private const val STORAGE_HEADROOM = 256L * 1024 * 1024
}

internal fun requiredNetworkType(wifiOnly: Boolean): NetworkType =
    if (wifiOnly) NetworkType.UNMETERED else NetworkType.CONNECTED
