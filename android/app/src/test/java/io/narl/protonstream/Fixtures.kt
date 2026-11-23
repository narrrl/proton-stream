package io.narl.protonstream

import uniffi.pstr_android.EpisodeRecord
import uniffi.pstr_android.SeasonRecord
import uniffi.pstr_android.TitleRecord
import uniffi.pstr_android.TitleType

/** An episode as the bridge hands it over, with what a test does not care about left empty. */
internal fun episodeRecord(
    link: String,
    number: UInt? = null,
    progress: Double? = null,
    resumeAt: Double? = progress?.let { it * 1_000 },
    watched: Boolean = false,
    lastPlayed: Long = 0,
    providerName: String? = null,
    stillUrl: String? = null,
    runtime: UInt? = null,
) = EpisodeRecord(
    shareId = "s", volumeId = "v", linkId = link, name = "[Group] Show - $link [1080p].mkv", label = "E$number",
    detail = "[Group] Show - $link [1080p].mkv", season = 1u, number = number, size = null, progress = progress,
    resumeAt = resumeAt, watched = watched, offline = false, providerName = providerName, providerOverview = null,
    stillUrl = stillUrl, airDate = null, lastPlayed = lastPlayed, runtime = runtime,
)

internal fun titleRecord(
    key: String,
    vararg episodes: EpisodeRecord,
    name: String = key,
    kind: TitleType = TitleType.SERIES,
    continueIndex: UInt? = null,
    posterUrl: String? = null,
    backdropUrl: String? = null,
    overview: String? = null,
) = TitleRecord(
    key = key, name = name, year = null, kind = kind, watchedCount = 0uL,
    episodeCount = episodes.size.toULong(), canonicalName = null, originalName = null, overview = overview,
    metadataProvider = null, metadataId = null, metadataYear = null, metadataKind = null, posterUrl = posterUrl,
    backdropUrl = backdropUrl, rating = null, genres = emptyList(), providerEpisodeCount = null, externalUrl = null,
    manualMatch = false, seasons = listOf(SeasonRecord(1u, "Season 1", episodes.toList())),
    displayName = name, wideUrl = null, formatLabel = null, seasonLabel = null, studios = emptyList(),
    directors = emptyList(), tags = emptyList(), airing = false, nextEpisode = null, nextAiringAt = null,
    franchise = emptyList(), continueIndex = continueIndex,
)
