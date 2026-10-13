package io.narl.protonstream.sync

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import io.narl.protonstream.MainActivity
import io.narl.protonstream.R
import io.narl.protonstream.native.NativeRuntime
import io.narl.protonstream.settings.SettingsStore
import uniffi.pstr_android.NewEpisodesRecord
import java.util.concurrent.TimeUnit

/**
 * Look at the shares while the app is away, and say when a show gained episodes.
 *
 * The app crawls a share it has not looked at for six hours whenever it opens,
 * which finds a new episode only once someone opens it to look. This runs the
 * same crawl on WorkManager's schedule instead. It only reports files the
 * crawl itself found, so a share the app crawled since says nothing twice.
 */
class NewEpisodesWorker(
    appContext: Context,
    parameters: WorkerParameters,
) : CoroutineWorker(appContext, parameters) {
    override suspend fun doWork(): Result {
        if (!SettingsStore(applicationContext).newEpisodeNotifications) return Result.success()
        val news = runCatching { NativeRuntime.engine().crawlForNews() }.getOrElse { return Result.retry() }
        if (news.isNotEmpty()) post(news)
        return Result.success()
    }

    private fun post(news: List<NewEpisodesRecord>) {
        if (
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            applicationContext.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            return
        }
        val manager = applicationContext.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_ID, "New episodes", NotificationManager.IMPORTANCE_DEFAULT).apply {
                description = "A show in your library gained episodes"
            },
        )
        val open = PendingIntent.getActivity(
            applicationContext,
            0,
            Intent(applicationContext, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val compat = NotificationManagerCompat.from(applicationContext)
        // One per show, keyed by it, so a second crawl's news for the same show
        // replaces the first rather than stacking under it.
        news.forEach { item ->
            val notification = NotificationCompat.Builder(applicationContext, CHANNEL_ID)
                .setSmallIcon(R.drawable.ic_notification_stream)
                .setContentTitle(item.title)
                .setContentText(describe(item))
                .setContentIntent(open)
                .setGroup(GROUP)
                .setAutoCancel(true)
                .build()
            compat.notify(item.titleKey, NOTIFICATION_ID, notification)
        }
        if (news.size > 1) {
            val summary = NotificationCompat.Builder(applicationContext, CHANNEL_ID)
                .setSmallIcon(R.drawable.ic_notification_stream)
                .setContentTitle("New episodes")
                .setContentText(news.joinToString { it.title })
                .setContentIntent(open)
                .setGroup(GROUP)
                .setGroupSummary(true)
                .setAutoCancel(true)
                .build()
            compat.notify(SUMMARY_TAG, NOTIFICATION_ID, summary)
        }
    }

    companion object {
        private const val WORK_NAME = "new-episodes"
        private const val CHANNEL_ID = "new-episodes"
        private const val GROUP = "io.narl.protonstream.NEW_EPISODES"
        private const val SUMMARY_TAG = "new-episodes-summary"
        private const val NOTIFICATION_ID = 7
        private const val INTERVAL_HOURS = 6L

        /** What one show's notification says: "S01E04 and 2 more". */
        fun describe(news: NewEpisodesRecord): String {
            val more = news.count.toInt() - 1
            val first = news.first
            return when {
                first == null -> if (news.count == 1u) "1 new episode" else "${news.count} new episodes"
                more == 0 -> "$first is new"
                more == 1 -> "$first and 1 more are new"
                else -> "$first and $more more are new"
            }
        }

        /**
         * Keep the periodic look scheduled, or cancel it when the viewer
         * turned it off. Called on every launch; keeping an existing schedule
         * means a launch does not push the next look back.
         */
        fun schedule(context: Context) {
            val work = WorkManager.getInstance(context.applicationContext)
            if (!SettingsStore(context).newEpisodeNotifications) {
                work.cancelUniqueWork(WORK_NAME)
                return
            }
            val request = PeriodicWorkRequestBuilder<NewEpisodesWorker>(INTERVAL_HOURS, TimeUnit.HOURS)
                .setConstraints(
                    Constraints.Builder()
                        .setRequiredNetworkType(NetworkType.CONNECTED)
                        .setRequiresBatteryNotLow(true)
                        .build(),
                )
                .build()
            work.enqueueUniquePeriodicWork(WORK_NAME, ExistingPeriodicWorkPolicy.KEEP, request)
        }
    }
}
