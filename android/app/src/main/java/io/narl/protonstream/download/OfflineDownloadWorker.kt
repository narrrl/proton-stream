package io.narl.protonstream.download

import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.pm.ServiceInfo
import android.content.Context
import androidx.core.app.NotificationCompat
import androidx.work.CoroutineWorker
import androidx.work.Data
import androidx.work.ForegroundInfo
import androidx.work.WorkerParameters
import androidx.work.workDataOf
import io.narl.protonstream.native.NativeRuntime
import uniffi.pstr_android.DownloadObserver
import java.util.UUID

/**
 * Durable boundary for native offline downloads. Rust owns the block-aligned transfer and
 * catalog mutation; WorkManager owns constraints, retries and user-visible lifecycle.
 */
class OfflineDownloadWorker(
    appContext: Context,
    parameters: WorkerParameters,
) : CoroutineWorker(appContext, parameters) {
    override suspend fun doWork(): Result {
        setForeground(createForegroundInfo(0))
        val shareId = inputData.getString(KEY_SHARE_ID) ?: return Result.failure(error("missing share"))
        val volumeId = inputData.getString(KEY_VOLUME_ID) ?: return Result.failure(error("missing volume"))
        val linkId = inputData.getString(KEY_LINK_ID) ?: return Result.failure(error("missing link"))
        val label = inputData.getString(KEY_LABEL) ?: linkId
        val store = DownloadStateStore(applicationContext)
        var retained = store.get(shareId, linkId)
            ?: RetainedDownload(shareId, volumeId, linkId, label)
        retained = retained.copy(status = RetainedDownload.STATUS_RUNNING, error = null)
        store.put(retained)

        val observer = object : DownloadObserver {
            override fun onProgress(downloaded: ULong, total: ULong) {
                val percent = if (total == 0UL) 0 else ((downloaded * 100UL) / total).toInt()
                setProgressAsync(workDataOf(KEY_DOWNLOADED to downloaded.toLong(), KEY_TOTAL to total.toLong()))
                setForegroundAsync(createForegroundInfo(percent))
                val requested = store.get(shareId, linkId)
                val requestedStatus = requested?.status
                retained = (requested ?: retained).copy(
                    downloaded = downloaded.toLong(),
                    total = total.toLong(),
                    status = if (requestedStatus == RetainedDownload.STATUS_PAUSED ||
                        requestedStatus == RetainedDownload.STATUS_CANCELLED
                    ) requestedStatus else RetainedDownload.STATUS_RUNNING,
                )
                store.put(retained)
            }

            override fun isCancelled(): Boolean = isStopped
        }
        return runCatching {
            NativeRuntime.engine().downloadEpisode(shareId, volumeId, linkId, observer)
        }.fold(
            onSuccess = {
                store.remove(shareId, linkId)
                Result.success()
            },
            onFailure = { failure ->
                if (isStopped) {
                    val current = store.get(shareId, linkId) ?: retained
                    if (current.status != RetainedDownload.STATUS_PAUSED) {
                        store.put(current.copy(status = RetainedDownload.STATUS_CANCELLED))
                    }
                    Result.failure(error("cancelled"))
                } else if (runAttemptCount < MAX_RETRIES) {
                    store.put(retained.copy(status = RetainedDownload.STATUS_QUEUED, error = failure.message))
                    Result.retry()
                } else {
                    val message = failure.message ?: "download failed"
                    store.put(retained.copy(status = RetainedDownload.STATUS_FAILED, error = message))
                    Result.failure(error(message))
                }
            },
        )
    }

    private fun createForegroundInfo(progress: Int): ForegroundInfo {
        val manager = applicationContext.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_ID, "Offline downloads", NotificationManager.IMPORTANCE_LOW),
        )
        val notification = NotificationCompat.Builder(applicationContext, CHANNEL_ID)
            .setSmallIcon(io.narl.protonstream.R.drawable.ic_launcher_foreground)
            .setContentTitle("Making episode available offline")
            .setProgress(100, progress, progress == 0)
            .setOngoing(true)
            .build()
        return ForegroundInfo(
            stableNotificationId(id),
            notification,
            ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
        )
    }

    private fun error(message: String) = Data.Builder().putString(KEY_ERROR, message).build()

    companion object {
        const val KEY_SHARE_ID = "share_id"
        const val KEY_VOLUME_ID = "volume_id"
        const val KEY_LINK_ID = "link_id"
        const val KEY_LABEL = "label"
        const val KEY_ERROR = "error"
        const val KEY_DOWNLOADED = "downloaded"
        const val KEY_TOTAL = "total"
        private const val MAX_RETRIES = 3
        private const val CHANNEL_ID = "offline-downloads"
    }
}

internal fun stableNotificationId(workId: UUID): Int =
    (workId.hashCode() and Int.MAX_VALUE).coerceAtLeast(1)
