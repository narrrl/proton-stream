package io.narl.protonstream.ui

import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.unit.Density
import androidx.test.core.app.ApplicationProvider
import androidx.work.testing.WorkManagerTestInitHelper
import com.github.takahirom.roborazzi.RobolectricDeviceQualifiers
import com.github.takahirom.roborazzi.captureRoboImage
import io.narl.protonstream.ui.theme.AppearanceState
import io.narl.protonstream.ui.theme.ProtonFallback
import io.narl.protonstream.ui.theme.ProtonStreamTheme
import io.narl.protonstream.ui.tv.HomeActions
import io.narl.protonstream.ui.tv.TitleActions
import io.narl.protonstream.ui.tv.TvHome
import io.narl.protonstream.ui.tv.TvSearch
import io.narl.protonstream.ui.tv.TvTheme
import io.narl.protonstream.ui.tv.TvTitle
import java.time.LocalDate
import java.time.ZoneOffset
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import uniffi.pstr_android.ArrangementRecord
import uniffi.pstr_android.EpisodeRecord
import uniffi.pstr_android.MetadataProvider
import uniffi.pstr_android.MetadataSettingsRecord
import uniffi.pstr_android.OfflineRecord
import uniffi.pstr_android.PaletteRecord
import uniffi.pstr_android.SeasonRecord
import uniffi.pstr_android.ShareRecord
import uniffi.pstr_android.ShelfRecord
import uniffi.pstr_android.StorageUsageRecord
import uniffi.pstr_android.TileRecord
import uniffi.pstr_android.CastRecord
import uniffi.pstr_android.TitleRecord
import uniffi.pstr_android.TitleType

/**
 * Every screen, drawn from fixed records, against a committed image.
 *
 * The screens are about to be redesigned one by one, and a layout regression —
 * a row that wraps, a button pushed off the edge — compiles and passes every
 * other test. Here it is a changed PNG in review. No artwork is loaded: the
 * bridge is not available on the host, so every tile shows its generated
 * placeholder, which is also what a title without metadata looks like on a
 * phone.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(qualifiers = RobolectricDeviceQualifiers.Pixel7)
class ScreenshotTest {
    @get:Rule val compose = createComposeRule()

    @Before
    fun setUp() {
        WorkManagerTestInitHelper.initializeTestWorkManager(ApplicationProvider.getApplicationContext())
        // The palette is process-wide and the sandbox outlives a test, so a
        // light capture would otherwise repaint every one after it.
        AppearanceState.apply(ProtonFallback)
    }

    private fun captureLight(name: String, content: @Composable () -> Unit) {
        AppearanceState.apply(PROTON_LIGHT)
        capture(name, content)
    }

    private fun capture(name: String, content: @Composable () -> Unit) {
        compose.setContent {
            ProtonStreamTheme {
                CompositionLocalProvider(LocalArtworkLoading provides false) {
                    Surface(color = MaterialTheme.colorScheme.background) { content() }
                }
            }
        }
        compose.onRoot().captureRoboImage("src/test/screenshots/$name.png")
    }

    @Test
    fun `the library on a phone`() = capture("library-phone") {
        LibraryScreen(MATCHED, {}, { _, _ -> }, {}, {}, { _, _ -> }, { _, _ -> }, PaddingValues())
    }

    @Test
    fun `the library offering posters while they are off`() = capture("library-phone-offer") {
        LibraryScreen(LIBRARY, {}, { _, _ -> }, {}, {}, { _, _ -> }, { _, _ -> }, PaddingValues())
    }

    @Test
    @Config(qualifiers = RobolectricDeviceQualifiers.MediumTablet)
    fun `the library on a tablet`() = capture("library-tablet") {
        LibraryScreen(MATCHED, {}, { _, _ -> }, {}, {}, { _, _ -> }, { _, _ -> }, PaddingValues())
    }

    @Test
    @Config(qualifiers = RobolectricDeviceQualifiers.Television1080p)
    fun `the television's home`() = capture("tv-home") {
        TvTheme { TvHome(MATCHED, TV_ACTIONS, { LazyListState() }, LazyListState(), null, {}) }
    }

    @Test
    @Config(qualifiers = RobolectricDeviceQualifiers.Television1080p)
    fun `a title on the television's own page`() = capture("tv-title") {
        TvTheme {
            TvTitle(
                SERIES,
                true,
                TitleActions({}, { _, _ -> }, { _, _ -> }, { _, _, _ -> }, {}),
                moreLike = MATCHED.titles.filter { it.key != SERIES.key },
            )
        }
    }

    @Test
    @Config(qualifiers = RobolectricDeviceQualifiers.Television1080p)
    fun `a search on a television`() = capture("tv-search") {
        TvTheme { TvSearch(MATCHED.copy(query = "o"), {}, TV_ACTIONS, {}, {}) }
    }

    @Test
    fun `the library in the light palette`() = captureLight("library-phone-light") {
        LibraryScreen(MATCHED, {}, { _, _ -> }, {}, {}, { _, _ -> }, { _, _ -> }, PaddingValues())
    }

    @Test
    fun `a title in the light palette`() = captureLight("title-phone-light") {
        TitleScreen(SERIES, true, { _, _ -> }, {}, {}, {}, { _, _ -> }, PaddingValues())
    }

    @Test
    @Config(qualifiers = RobolectricDeviceQualifiers.MediumTablet)
    fun `a title on a tablet`() = capture("title-tablet") {
        TitleScreen(SERIES, true, { _, _ -> }, {}, {}, {}, { _, _ -> }, PaddingValues())
    }

    @Test
    @Config(qualifiers = RobolectricDeviceQualifiers.MediumTablet)
    fun `the history page on a tablet`() = capture("history-tablet") {
        HistoryScreen(LIBRARY, { _, _ -> }, {}, { _, _ -> }, PaddingValues(), ZoneOffset.UTC, LocalDate.of(2026, 9, 21))
    }

    @Test
    fun `the library at a large font scale`() = capture("library-phone-large-font") {
        // Robolectric has no font-scale qualifier; the density is what Compose
        // reads it from, so the scale is set there.
        val density = LocalDensity.current
        CompositionLocalProvider(LocalDensity provides Density(density.density, fontScale = 1.5f)) {
            LibraryScreen(MATCHED, {}, { _, _ -> }, {}, {}, { _, _ -> }, { _, _ -> }, PaddingValues())
        }
    }

    @Test
    @Config(qualifiers = "w360dp-h640dp-xhdpi")
    fun `a title at twice the font size on a small phone`() = capture("title-small-phone-huge-font") {
        val density = LocalDensity.current
        CompositionLocalProvider(LocalDensity provides Density(density.density, fontScale = 2f)) {
            TitleScreen(SERIES, true, { _, _ -> }, {}, {}, {}, { _, _ -> }, PaddingValues(), franchise = listOf(SERIES, SEQUEL))
        }
    }

    @Test
    fun `an empty library`() = capture("library-empty") {
        LibraryScreen(AppUiState(loading = false), {}, { _, _ -> }, {}, {}, { _, _ -> }, { _, _ -> }, PaddingValues())
    }

    @Test
    fun `the library while the catalog loads`() = capture("library-loading") {
        LibraryScreen(AppUiState(), {}, { _, _ -> }, {}, {}, { _, _ -> }, { _, _ -> }, PaddingValues())
    }

    @Test
    fun `a title with seasons`() = capture("title-phone") {
        TitleScreen(SERIES, true, { _, _ -> }, {}, {}, {}, { _, _ -> }, PaddingValues(), franchise = listOf(SERIES, SEQUEL))
    }

    @Test
    fun `the shares page`() = capture("shares-phone") {
        SharesScreen(LIBRARY.shares, { _, _, _ -> null }, { _, _, _ -> }, {}, {}, null, {}, PaddingValues())
    }

    @Test
    fun `the settings page`() = capture("settings-phone") {
        SettingsScreen(LIBRARY, { _, _, _, _ -> null }, {}, {}, {}, PaddingValues(), version = "1.0.0")
    }

    @Test
    fun `the playback settings`() = capture("settings-playback") {
        SettingsScreen(LIBRARY, { _, _, _, _ -> null }, {}, {}, {}, PaddingValues(), SettingsPage.Playback)
    }

    @Test
    fun `the storage settings`() = capture("settings-storage") {
        SettingsScreen(LIBRARY, { _, _, _, _ -> null }, {}, {}, {}, PaddingValues(), SettingsPage.Storage)
    }

    @Test
    fun `the history page`() = capture("history-phone") {
        // Fixed day and zone: "Today" and "Yesterday" are relative, and an
        // image that changes with the date is no baseline.
        HistoryScreen(LIBRARY, { _, _ -> }, {}, { _, _ -> }, PaddingValues(), ZoneOffset.UTC, LocalDate.of(2026, 9, 21))
    }

    @Test
    fun `the downloads page with a season saved`() = capture("downloads-phone") {
        val saved = SERIES.seasons.first().episodes.take(2).map {
            OfflineRecord(it.shareId, it.linkId, "revision", 1_300_000_000uL, it)
        }
        // One title: the fixtures reuse link ids across titles, so with the
        // whole library the episodes would be filed under whichever came last.
        DownloadsScreen(LIBRARY.copy(titles = listOf(SERIES), offline = saved), {}, {}, {}, {}, PaddingValues())
    }

    @Test
    fun `the downloads page with nothing offline`() = capture("downloads-empty") {
        DownloadsScreen(LIBRARY, {}, {}, {}, {}, PaddingValues())
    }

    internal companion object {
        /** A home page whose every action goes nowhere. */
        val TV_ACTIONS = HomeActions({ _, _ -> }, {}, { _, _ -> }, { _, _ -> }, { _, _ -> }, {}, {})

        /** Proton Light with the default accent, as `Palette::resolve` gives it. */
        val PROTON_LIGHT = PaletteRecord(
            background = 0xFFFAF9FCu,
            surface = 0xFFF4F1FAu,
            sunken = 0xFFEBE7F3u,
            card = 0xFFF1EEF8u,
            cardHover = 0xFFE3DEEDu,
            border = 0xFFE3DEEDu,
            text = 0xFF1B1340u,
            muted = 0xFF615970u,
            accent = 0xFF6D4AFFu,
            accentAlt = 0xFFB65BD6u,
            accentDim = 0xFFCBC4FDu,
            onAccent = 0xFFFAF9FCu,
            danger = 0xFFB42339u,
            elevated = 0xFFD4CCE1u,
            dangerDim = 0xFFE9D8DCu,
            highlight = 0xFFB47456u,
            light = true,
        )

        fun episode(season: UInt, number: UInt, watched: Boolean = false, progress: Double? = null): EpisodeRecord {
            // `detail` is the filename unless the filename named the episode,
            // which is what the bridge sends.
            val file = "[Group] Oshi no Ko - S%02dE%02d.mkv".format(season.toInt(), number.toInt())
            return episodeRecord(season, number, file, watched, progress)
        }

        fun episodeRecord(season: UInt, number: UInt, file: String, watched: Boolean, progress: Double?) = EpisodeRecord(
            shareId = "share",
            volumeId = "volume",
            linkId = "s${season}e$number",
            name = file,
            label = "S%02dE%02d".format(season.toInt(), number.toInt()),
            detail = file,
            season = season,
            number = number,
            size = 1_300_000_000uL,
            progress = progress,
            resumeAt = progress?.let { it * 1440 },
            watched = watched,
            offline = false,
            providerName = if (number == 1u) "Mother and Children" else null,
            providerOverview = null,
            stillUrl = null,
            airDate = null,
            // 2026-09-21 for the part-watched one, the day before for the rest.
            lastPlayed = when {
                progress != null -> 1_790_000_000L
                watched -> 1_789_900_000L - number.toLong() * 600
                else -> 0L
            },
        )

        fun title(key: String, name: String, seasons: List<SeasonRecord>, overview: String? = null) = TitleRecord(
            key = key,
            name = name,
            year = 2023u,
            kind = TitleType.SERIES,
            watchedCount = seasons.sumOf { s -> s.episodes.count { it.watched } }.toULong(),
            episodeCount = seasons.sumOf { it.episodes.size }.toULong(),
            canonicalName = null,
            originalName = null,
            overview = overview,
            metadataProvider = null,
            metadataId = null,
            metadataYear = null,
            metadataKind = null,
            posterUrl = null,
            backdropUrl = null,
            rating = 8.4,
            genres = listOf("Drama", "Mystery"),
            providerEpisodeCount = null,
            externalUrl = null,
            manualMatch = false,
            seasons = seasons,
            displayName = name,
            wideUrl = null,
            formatLabel = null,
            seasonLabel = null,
            studios = emptyList(),
            directors = emptyList(),
            tags = emptyList(),
            airing = false,
            nextEpisode = null,
            nextAiringAt = null,
            franchise = emptyList(),
            // What the engine would pick for these fixtures: the part-watched episode.
            continueIndex = seasons.flatMap { it.episodes }.indexOfFirst { it.resumeAt != null }
                .takeIf { it >= 0 }?.toUInt(),
        )

        val SERIES = title(
            "oshi-no-ko",
            "Oshi no Ko",
            listOf(
                SeasonRecord(1u, "Season 1", (1u..4u).map { episode(1u, it, watched = it < 3u) }),
                SeasonRecord(2u, "Season 2", (1u..3u).map { episode(2u, it, progress = if (it == 1u) 0.4 else null) }),
            ),
            overview = "A doctor in a countryside clinic is reborn as the child of the idol he followed.",
        ).copy(
            formatLabel = "TV",
            seasonLabel = "Spring 2023",
            studios = listOf("Doga Kobo"),
            directors = listOf("Daisuke Hiramaki"),
            tags = listOf("Idol", "Reincarnation"),
            franchise = listOf("oshi-no-ko", "oshi-no-ko-film"),
            trailerUrl = "https://www.youtube.com/watch?v=trailer",
            cast = listOf(
                CastRecord("Ai Hoshino", null, "Rie Takahashi", null),
                CastRecord("Aqua", null, "Takeo Ootsuka", null),
            ),
        )

        /** The rest of [SERIES]'s franchise, for the title page's row of it. */
        val SEQUEL = title("oshi-no-ko-film", "Oshi no Ko: The Final Act", listOf(SeasonRecord(null, "Film", listOf(episode(1u, 1u)))))
            .copy(kind = TitleType.FILM, year = 2024u, formatLabel = "Film", franchise = SERIES.franchise)

        val FILM = title("akira", "Akira", listOf(SeasonRecord(null, "Film", listOf(episode(1u, 1u)))))
            .copy(kind = TitleType.FILM, year = 1988u)

        /** [LIBRARY] with posters on, as the shelf above its grid implies. */
        val MATCHED get() = LIBRARY.copy(
            metadataSettings = MetadataSettingsRecord(
                enabled = true,
                provider = MetadataProvider.ANI_LIST,
                language = "en",
                ready = true,
            ),
        )

        val LIBRARY = AppUiState(
            loading = false,
            titles = listOf(
                FILM,
                SERIES,
                title("ano-hana", "Ano Hi Mita Hana no Namae o Bokutachi wa Mada Shiranai.", listOf(SeasonRecord(1u, "Season 1", (1u..11u).map { episode(1u, it) }))),
                title("bleach", "Bleach", listOf(SeasonRecord(1u, "Season 1", (1u..2u).map { episode(1u, it) }))),
            ),
            // Bleach stands for one more title of its franchise, and one shelf
            // sits above the grid: what the bridge's arrangement would give.
            arrangement = ArrangementRecord(
                tiles = listOf("akira", "ano-hana", "bleach", "oshi-no-ko").map {
                    TileRecord(it, if (it == "bleach") 1u else 0u)
                },
                shelves = listOf(ShelfRecord("From MAPPA", listOf("oshi-no-ko", "bleach", "akira"))),
            ),
            // Crawled relative to the clock, so the row reads "2 h ago" on
            // whatever day the test runs.
            shares = listOf(
                ShareRecord(
                    "anime",
                    "anime",
                    true,
                    crawledAt = System.currentTimeMillis() / 1_000 - 2 * 3_600,
                    crawlFiles = 120uL,
                ),
                ShareRecord("films", "films", false, crawlError = "the link was revoked or has expired"),
            ),
            storage = StorageUsageRecord(6_400_000_000uL, 3uL, 0uL, 1_100_000_000uL),
        )
    }
}
