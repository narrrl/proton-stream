package io.narl.protonstream.sync

import android.content.Context
import androidx.work.BackoffPolicy
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import io.narl.protonstream.native.NativeRuntime
import uniffi.pstr_android.AccountState
import java.util.concurrent.TimeUnit

/**
 * One watch-history sync after the app leaves the screen.
 *
 * The model's timer only runs while the app is alive, and Android is free to
 * kill a process in the background without notice — so the position the
 * viewer stopped at would wait for the next launch to reach their other
 * devices. Handed to WorkManager instead, it goes as soon as there is a
 * network, even if the process is gone by then.
 */
class WatchSyncWorker(
    appContext: Context,
    parameters: WorkerParameters,
) : CoroutineWorker(appContext, parameters) {
    override suspend fun doWork(): Result {
        val engine = NativeRuntime.engine()
        return runCatching { engine.syncWatchHistory() }.fold(
            onSuccess = { Result.success() },
            onFailure = {
                // Signed out by the engine — the session ended — is not
                // something a retry fixes; a dropped connection is.
                val signedOut = runCatching { engine.accountState() }.getOrNull() == AccountState.SignedOut
                if (signedOut || runAttemptCount >= MAX_ATTEMPTS) Result.failure() else Result.retry()
            },
        )
    }

    companion object {
        private const val WORK_NAME = "watch-history-sync"
        private const val MAX_ATTEMPTS = 3

        /** Replacing: one pending sync covers every position saved before it runs. */
        fun enqueue(context: Context) {
            val request = OneTimeWorkRequestBuilder<WatchSyncWorker>()
                .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
                .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, 30, TimeUnit.SECONDS)
                .build()
            WorkManager.getInstance(context.applicationContext)
                .enqueueUniqueWork(WORK_NAME, ExistingWorkPolicy.REPLACE, request)
        }
    }
}
