package io.narl.protonstream.download

import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.pm.ServiceInfo
import android.content.Context
import android.os.SystemClock
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
        createNotificationChannel()
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
            private var reportedAt = 0L
            private var reportedPercent = -1

            /**
             * Called once per 4 MiB block — a dozen times a second on a fast
             * link, each one a WorkManager database write, a notification post
             * and a preferences write. Nothing the viewer can see changes that
             * often, so a report is only made when the percentage moves or a
             * second has passed. The final one is never suppressed.
             */
            override fun onProgress(downloaded: ULong, total: ULong) {
                val percent = if (total == 0UL) 0 else ((downloaded * 100UL) / total).toInt()
                val now = SystemClock.elapsedRealtime()
                val complete = total > 0UL && downloaded >= total
                if (!complete && percent == reportedPercent && now - reportedAt < PROGRESS_INTERVAL_MS) {
                    return
                }
                reportedAt = now
                reportedPercent = percent
                setProgressAsync(workDataOf(KEY_DOWNLOADED to downloaded.toLong(), KEY_TOTAL to total.toLong()))
                setForegroundAsync(createForegroundInfo(percent))
                // Under the store's lock: `pause()` is a concurrent writer, and
                // this read-modify-write used to put RUNNING back over the
                // viewer's PAUSED and then report the download as cancelled.
                store.update(shareId, linkId) { stored ->
                    val current = stored ?: retained
                    current.copy(
                        downloaded = downloaded.toLong(),
                        total = total.toLong(),
                        status = when (current.status) {
                            RetainedDownload.STATUS_PAUSED,
                            RetainedDownload.STATUS_CANCELLED,
                            -> current.status
                            else -> RetainedDownload.STATUS_RUNNING
                        },
                    ).also { retained = it }
                }
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
                    // A pause is also a stop. Under the lock, so the record read
                    // here is the one the write lands on.
                    store.update(shareId, linkId) { stored ->
                        val current = stored ?: retained
                        current.takeIf { it.status != RetainedDownload.STATUS_PAUSED }
                            ?.copy(status = RetainedDownload.STATUS_CANCELLED)
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

    /** Once per run, rather than once per 4 MiB block. */
    private fun createNotificationChannel() {
        applicationContext.getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL_ID, "Offline downloads", NotificationManager.IMPORTANCE_LOW),
        )
    }

    private fun createForegroundInfo(progress: Int): ForegroundInfo {
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

        /** Shortest gap between two identical-looking progress reports. */
        private const val PROGRESS_INTERVAL_MS = 1_000L
    }
}

internal fun stableNotificationId(workId: UUID): Int =
    (workId.hashCode() and Int.MAX_VALUE).coerceAtLeast(1)
