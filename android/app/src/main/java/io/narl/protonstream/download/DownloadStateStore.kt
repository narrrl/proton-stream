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

    fun put(record: RetainedDownload) {
        preferences.edit().putString(record.key, encode(record)).commit()
    }

    fun remove(shareId: String, linkId: String) {
        preferences.edit().remove(key(shareId, linkId)).commit()
    }

    fun removeShare(shareId: String) {
        val edit = preferences.edit()
        records().filter { it.shareId == shareId }.forEach { edit.remove(it.key) }
        edit.commit()
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
        )
    }

    private fun key(shareId: String, linkId: String) = "$shareId\u001f$linkId"

    private companion object {
        const val NAME = "offline_download_state"
    }
}
