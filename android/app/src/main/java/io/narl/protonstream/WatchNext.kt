package io.narl.protonstream

import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.media.tv.TvContract
import android.media.tv.TvContract.WatchNextPrograms
import android.provider.BaseColumns
import android.util.Log
import io.narl.protonstream.ui.continueWatching
import io.narl.protonstream.ui.shownName
import uniffi.pstr_android.TitleRecord
import uniffi.pstr_android.TitleType

/**
 * One card of the television's Watch Next row: what Continue watching offers
 * in the app, for the launcher's home screen.
 *
 * Only what the card shows is carried: the show's name, the episode's own
 * name and numbers, artwork the metadata provider published and the synopsis.
 * Never the file's name, which says where the library came from.
 */
internal data class WatchNextEntry(
    /** `share/link`: how a row is found again ([WatchNextPrograms.COLUMN_INTERNAL_PROVIDER_ID]). */
    val key: String,
    val titleKey: String,
    val linkId: String,
    val film: Boolean,
    val title: String,
    val episodeTitle: String?,
    val season: Int?,
    val episode: Int?,
    /** Part-watched, rather than the episode after one finished. */
    val started: Boolean,
    val positionMs: Long?,
    val durationMs: Long?,
    val engagedAtMs: Long,
    val artUrl: String?,
    val wideArt: Boolean,
    val description: String?,
)

/** Continue watching, as Watch Next cards. */
internal fun watchNextEntries(titles: List<TitleRecord>): List<WatchNextEntry> =
    continueWatching(titles).map { (title, episode, _, lastPlayed) ->
        val film = title.kind == TitleType.FILM
        val started = episode.resumeAt != null
        val duration = episode.resumeAt
            ?.let { at -> episode.progress?.takeIf { it > 0.0 && it < 1.0 }?.let { at / it } }
            ?: episode.runtime?.let { it.toDouble() * 60 }
            ?: title.runtime?.let { it.toDouble() * 60 }
        val wide = listOf(if (film) null else episode.stillUrl, title.wideUrl, title.backdropUrl).firstOrNull(::isWebUrl)
        WatchNextEntry(
            key = "${episode.shareId}/${episode.linkId}",
            titleKey = title.key,
            linkId = episode.linkId,
            film = film,
            title = title.displayName,
            episodeTitle = if (film) null else episode.shownName() ?: episode.label.takeIf { episode.number == null },
            season = if (film) null else episode.season?.toInt(),
            episode = if (film) null else episode.number?.toInt(),
            started = started,
            positionMs = episode.resumeAt?.let { (it * 1000).toLong() },
            durationMs = duration?.let { (it * 1000).toLong() },
            engagedAtMs = lastPlayed * 1000,
            artUrl = wide ?: title.posterUrl?.takeIf(::isWebUrl),
            wideArt = wide != null,
            description = (if (film) null else episode.providerOverview) ?: title.overview,
        )
    }

private fun isWebUrl(url: String?): Boolean = url != null && (url.startsWith("https://") || url.startsWith("http://"))

/** The launcher's columns for [entry]: what [WatchNextPublisher] writes. */
internal fun watchNextColumns(entry: WatchNextEntry, intentUri: String): Map<String, Any?> = buildMap {
    put(WatchNextPrograms.COLUMN_INTERNAL_PROVIDER_ID, entry.key)
    put(WatchNextPrograms.COLUMN_TYPE, if (entry.film) WatchNextPrograms.TYPE_MOVIE else WatchNextPrograms.TYPE_TV_EPISODE)
    put(
        WatchNextPrograms.COLUMN_WATCH_NEXT_TYPE,
        if (entry.started) WatchNextPrograms.WATCH_NEXT_TYPE_CONTINUE else WatchNextPrograms.WATCH_NEXT_TYPE_NEXT,
    )
    put(WatchNextPrograms.COLUMN_TITLE, entry.title)
    put(WatchNextPrograms.COLUMN_EPISODE_TITLE, entry.episodeTitle)
    put(WatchNextPrograms.COLUMN_SEASON_DISPLAY_NUMBER, entry.season?.toString())
    put(WatchNextPrograms.COLUMN_EPISODE_DISPLAY_NUMBER, entry.episode?.toString())
    put(WatchNextPrograms.COLUMN_SHORT_DESCRIPTION, entry.description)
    put(WatchNextPrograms.COLUMN_POSTER_ART_URI, entry.artUrl)
    put(
        WatchNextPrograms.COLUMN_POSTER_ART_ASPECT_RATIO,
        if (entry.wideArt) WatchNextPrograms.ASPECT_RATIO_16_9 else WatchNextPrograms.ASPECT_RATIO_2_3,
    )
    // A position is only a position against a length: without one the card
    // would draw an empty progress bar.
    if (entry.started && entry.durationMs != null) {
        put(WatchNextPrograms.COLUMN_LAST_PLAYBACK_POSITION_MILLIS, entry.positionMs?.toInt())
    }
    put(WatchNextPrograms.COLUMN_DURATION_MILLIS, entry.durationMs?.toInt())
    put(WatchNextPrograms.COLUMN_LAST_ENGAGEMENT_TIME_UTC_MILLIS, entry.engagedAtMs)
    put(WatchNextPrograms.COLUMN_INTENT_URI, intentUri)
}

/** A row this app already has in the launcher's Watch Next. */
internal data class WatchNextRow(val rowId: Long, val key: String?, val browsable: Boolean, val engagedAtMs: Long)

internal sealed interface WatchNextChange {
    data class Insert(val entry: WatchNextEntry) : WatchNextChange
    data class Update(val rowId: Long, val entry: WatchNextEntry) : WatchNextChange
    data class Delete(val rowId: Long) : WatchNextChange
}

/**
 * How to turn [rows] into [entries].
 *
 * A row the viewer removed from the home screen stays behind, not browsable,
 * and stays off it until they watch that episode again: the launcher's rule,
 * and the only way to tell "removed" from "never added". An app may not make
 * a row browsable again, so watching it again replaces the row.
 */
internal fun planWatchNext(rows: List<WatchNextRow>, entries: List<WatchNextEntry>): List<WatchNextChange> {
    val wanted = entries.associateBy { it.key }
    val kept = mutableSetOf<String>()
    val changes = mutableListOf<WatchNextChange>()
    for (row in rows) {
        val entry = row.key?.let(wanted::get)
        when {
            entry == null || row.key in kept -> changes += WatchNextChange.Delete(row.rowId)
            !row.browsable && entry.engagedAtMs <= row.engagedAtMs -> kept += row.key
            !row.browsable -> {
                changes += WatchNextChange.Delete(row.rowId)
                changes += WatchNextChange.Insert(entry)
                kept += row.key
            }
            else -> {
                changes += WatchNextChange.Update(row.rowId, entry)
                kept += row.key
            }
        }
    }
    entries.filter { it.key !in kept }.forEach { changes += WatchNextChange.Insert(it) }
    return changes
}

/**
 * Keeps the television's Watch Next row in step with Continue watching.
 *
 * Only on a television, and quietly: a set without the TV provider, or one
 * that refuses the write, just has no row, which is not worth telling anyone.
 */
internal class WatchNextPublisher(private val context: Context) {
    private val resolver get() = context.contentResolver

    fun publish(entries: List<WatchNextEntry>) {
        if (!context.packageManager.hasSystemFeature(PackageManager.FEATURE_LEANBACK)) return
        runCatching {
            planWatchNext(rows(), entries).forEach(::apply)
        }.onFailure { Log.w(TAG, "Watch Next was not updated", it) }
    }

    private fun rows(): List<WatchNextRow> {
        val columns = arrayOf(
            BaseColumns._ID,
            WatchNextPrograms.COLUMN_INTERNAL_PROVIDER_ID,
            WatchNextPrograms.COLUMN_BROWSABLE,
            WatchNextPrograms.COLUMN_LAST_ENGAGEMENT_TIME_UTC_MILLIS,
        )
        // The provider only ever answers an app with its own rows.
        return resolver.query(WatchNextPrograms.CONTENT_URI, columns, null, null, null)?.use { cursor ->
            buildList {
                while (cursor.moveToNext()) {
                    add(
                        WatchNextRow(
                            rowId = cursor.getLong(0),
                            key = cursor.getString(1),
                            browsable = cursor.getInt(2) != 0,
                            engagedAtMs = cursor.getLong(3),
                        ),
                    )
                }
            }
        }.orEmpty()
    }

    private fun apply(change: WatchNextChange) {
        when (change) {
            is WatchNextChange.Insert -> resolver.insert(WatchNextPrograms.CONTENT_URI, values(change.entry))
            is WatchNextChange.Update ->
                resolver.update(TvContract.buildWatchNextProgramUri(change.rowId), values(change.entry), null, null)
            is WatchNextChange.Delete ->
                resolver.delete(TvContract.buildWatchNextProgramUri(change.rowId), null, null)
        }
    }

    private fun values(entry: WatchNextEntry): ContentValues {
        val intent = playEpisodeIntent(context, entry.titleKey, entry.linkId).toUri(Intent.URI_INTENT_SCHEME)
        return ContentValues().apply {
            watchNextColumns(entry, intent).forEach { (column, value) ->
                when (value) {
                    null -> putNull(column)
                    is String -> put(column, value)
                    is Int -> put(column, value)
                    is Long -> put(column, value)
                    else -> error("no column type for $value")
                }
            }
        }
    }

    private companion object {
        const val TAG = "WatchNext"
    }
}
