package io.narl.protonstream.download

import android.content.Context
import org.json.JSONObject

/** Durable user-facing state retained after WorkManager prunes finished work. */
data class RetainedDownload(
    val shareId: String,
    val volumeId: String,
    val linkId: String,
    val label: String,
    val downloaded: Long = 0,
    val total: Long = 0,
    val status: String = STATUS_QUEUED,
    val error: String? = null,
    /** Recent transfer rate while running; 0 when unknown or not running. */
    val bytesPerSecond: Long = 0,
) {
    val key: String get() = "$shareId\u001f$linkId"

    companion object {
        const val STATUS_QUEUED = "queued"
        const val STATUS_RUNNING = "running"
        const val STATUS_PAUSED = "paused"
        const val STATUS_FAILED = "failed"
        const val STATUS_CANCELLED = "cancelled"
    }
}

class DownloadStateStore(context: Context) {
    private val preferences = context.getSharedPreferences(NAME, Context.MODE_PRIVATE)

    fun records(): List<RetainedDownload> = preferences.all.values.mapNotNull { encoded ->
        runCatching { decode(encoded as String) }.getOrNull()
    }.sortedBy { it.label.lowercase() }

    fun get(shareId: String, linkId: String): RetainedDownload? =
        preferences.getString(key(shareId, linkId), null)?.let { runCatching { decode(it) }.getOrNull() }

    fun put(record: RetainedDownload) = synchronized(WRITES) {
        edit { putString(record.key, encode(record)) }
    }

    /**
     * Read-modify-write one record, excluding every other writer.
     *
     * The worker's progress update reads the stored record to decide whether the
     * viewer has paused, then writes it back. `pause()` is another writer, and
     * without this a `put` landing between those two halves puts `RUNNING` back
     * over `PAUSED` — after which the worker's own cancellation handler sees a
     * status that is not `PAUSED` and records `CANCELLED`. The pause is simply
     * lost, which is the visible bug.
     *
     * Returning null from [change] leaves the record alone.
     */
    fun update(shareId: String, linkId: String, change: (RetainedDownload?) -> RetainedDownload?) =
        synchronized(WRITES) {
            change(get(shareId, linkId))?.let { edit { putString(it.key, encode(it)) } }
        }

    /**
     * Write a whole queue in one go.
     *
     * Queueing a season is one call, not one per episode: `commit()` per record
     * on a 200-episode series was 200 synchronous fsyncs, and the callers are
     * click handlers.
     */
    fun putAll(records: Collection<RetainedDownload>) = synchronized(WRITES) {
        edit { records.forEach { putString(it.key, encode(it)) } }
    }

    fun remove(shareId: String, linkId: String) = synchronized(WRITES) {
        edit { remove(key(shareId, linkId)) }
    }

    /** Forget every retained download. Callers cancel the work first. */
    fun clear() = synchronized(WRITES) { edit { clear() } }

    fun removeShare(shareId: String) = synchronized(WRITES) {
        edit { records().filter { it.shareId == shareId }.forEach { remove(it.key) } }
    }

    /**
     * `apply()`, not `commit()`.
     *
     * The in-memory map is updated before this returns, so a read that follows a
     * write in the same process still sees it; only the disk write is deferred,
     * and losing the newest progress reading to a kill is not worth an fsync on
     * whichever thread happened to be writing.
     */
    private inline fun edit(block: android.content.SharedPreferences.Editor.() -> Unit) {
        preferences.edit().apply(block).apply()
    }

    private fun encode(record: RetainedDownload) = JSONObject()
        .put("share", record.shareId)
        .put("volume", record.volumeId)
        .put("link", record.linkId)
        .put("label", record.label)
        .put("downloaded", record.downloaded)
        .put("total", record.total)
        .put("status", record.status)
        .put("error", record.error)
        .put("rate", record.bytesPerSecond)
        .toString()

    private fun decode(encoded: String): RetainedDownload {
        val json = JSONObject(encoded)
        return RetainedDownload(
            shareId = json.getString("share"),
            volumeId = json.getString("volume"),
            linkId = json.getString("link"),
            label = json.optString("label", json.getString("link")),
            downloaded = json.optLong("downloaded"),
            total = json.optLong("total"),
            status = json.optString("status", RetainedDownload.STATUS_QUEUED),
            error = json.optString("error").takeIf(String::isNotBlank),
            bytesPerSecond = json.optLong("rate"),
        )
    }

    private fun key(shareId: String, linkId: String) = "$shareId\u001f$linkId"

    private companion object {
        const val NAME = "offline_download_state"

        /**
         * One lock for the whole process. Instances of this class are created
         * ad hoc from a `Context`, so the lock cannot live on one of them.
         */
        val WRITES = Any()
    }
}
