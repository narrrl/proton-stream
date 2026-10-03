package io.narl.protonstream.ui

import androidx.compose.foundation.layout.PaddingValues
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
import io.narl.protonstream.ui.theme.ProtonStreamTheme
import java.time.LocalDate
import java.time.ZoneOffset
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import uniffi.pstr_android.EpisodeRecord
import uniffi.pstr_android.SeasonRecord
import uniffi.pstr_android.ShareRecord
import uniffi.pstr_android.StorageUsageRecord
import uniffi.pstr_android.TitleRecord
import uniffi.pstr_android.TitleType

/**
 * Every screen, drawn from fixed records, against a committed image.
 *
 * The screens are about to be redesigned one by one, and a layout regression —
 * a row that wraps, a button pushed off the edge — compiles and passes every
 * other test. Here it is a changed PNG in review. No artwork is loaded: the
 * bridge is not available on the host, so every tile shows its placeholder,
 * which is also what a title without metadata looks like on a phone.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(qualifiers = RobolectricDeviceQualifiers.Pixel7)
class ScreenshotTest {
    @get:Rule val compose = createComposeRule()

    @Before
    fun setUp() {
        WorkManagerTestInitHelper.initializeTestWorkManager(ApplicationProvider.getApplicationContext())
    }

    private fun capture(name: String, content: @Composable () -> Unit) {
        compose.setContent {
            ProtonStreamTheme {
                Surface(color = MaterialTheme.colorScheme.background) { content() }
            }
        }
        compose.onRoot().captureRoboImage("src/test/screenshots/$name.png")
    }

    @Test
    fun `the library on a phone`() = capture("library-phone") {
        LibraryScreen(LIBRARY, {}, { _, _ -> }, {}, {}, { _, _ -> }, { _, _ -> }, PaddingValues())
    }

    @Test
    @Config(qualifiers = RobolectricDeviceQualifiers.MediumTablet)
    fun `the library on a tablet`() = capture("library-tablet") {
        LibraryScreen(LIBRARY, {}, { _, _ -> }, {}, {}, { _, _ -> }, { _, _ -> }, PaddingValues())
    }

    @Test
    fun `the library at a large font scale`() = capture("library-phone-large-font") {
        // Robolectric has no font-scale qualifier; the density is what Compose
        // reads it from, so the scale is set there.
        val density = LocalDensity.current
        CompositionLocalProvider(LocalDensity provides Density(density.density, fontScale = 1.5f)) {
            LibraryScreen(LIBRARY, {}, { _, _ -> }, {}, {}, { _, _ -> }, { _, _ -> }, PaddingValues())
        }
    }

    @Test
    fun `an empty library`() = capture("library-empty") {
        LibraryScreen(AppUiState(loading = false), {}, { _, _ -> }, {}, {}, { _, _ -> }, { _, _ -> }, PaddingValues())
    }

    @Test
    fun `a title with seasons`() = capture("title-phone") {
        TitleScreen(SERIES, true, { _, _ -> }, {}, {}, {}, { _, _ -> }, PaddingValues())
    }

    @Test
    fun `the shares page`() = capture("shares-phone") {
        SharesScreen(LIBRARY.shares, { _, _, _ -> }, { _, _, _ -> }, {}, {}, null, {}, PaddingValues())
    }

    @Test
    fun `the settings page`() = capture("settings-phone") {
        SettingsScreen(LIBRARY, { _, _, _, _ -> }, {}, {}, {}, PaddingValues())
    }

    @Test
    fun `the history page`() = capture("history-phone") {
        // Fixed day and zone: "Today" and "Yesterday" are relative, and an
        // image that changes with the date is no baseline.
        HistoryScreen(LIBRARY, { _, _ -> }, {}, { _, _ -> }, PaddingValues(), ZoneOffset.UTC, LocalDate.of(2026, 9, 21))
    }

    @Test
    fun `the downloads page with nothing offline`() = capture("downloads-empty") {
        DownloadsScreen(LIBRARY, {}, {}, {}, {}, PaddingValues())
    }

    private companion object {
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
        )

        val SERIES = title(
            "oshi-no-ko",
            "Oshi no Ko",
            listOf(
                SeasonRecord(1u, "Season 1", (1u..4u).map { episode(1u, it, watched = it < 3u) }),
                SeasonRecord(2u, "Season 2", (1u..3u).map { episode(2u, it, progress = if (it == 1u) 0.4 else null) }),
            ),
            overview = "A doctor in a countryside clinic is reborn as the child of the idol he followed.",
        )

        val FILM = title("akira", "Akira", listOf(SeasonRecord(null, "Film", listOf(episode(1u, 1u)))))
            .copy(kind = TitleType.FILM, year = 1988u)

        val LIBRARY = AppUiState(
            loading = false,
            titles = listOf(
                FILM,
                SERIES,
                title("ano-hana", "Ano Hi Mita Hana no Namae o Bokutachi wa Mada Shiranai.", listOf(SeasonRecord(1u, "Season 1", (1u..11u).map { episode(1u, it) }))),
                title("bleach", "Bleach", listOf(SeasonRecord(1u, "Season 1", (1u..2u).map { episode(1u, it) }))),
            ),
            shares = listOf(
                ShareRecord("anime", "anime", true),
                ShareRecord("films", "films", false),
            ),
            storage = StorageUsageRecord(6_400_000_000uL, 3uL, 0uL, 1_100_000_000uL),
        )
    }
}
