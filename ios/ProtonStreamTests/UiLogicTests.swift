@testable import ProtonStream
import SwiftUI
import XCTest

private func episode(_ link: String, watched: Bool = false, progress: Double? = nil, lastPlayed: Int64 = 0, number: UInt32? = nil) -> EpisodeRecord {
    EpisodeRecord(
        shareId: "s", volumeId: "v", linkId: link, name: "\(link).mkv", label: link, detail: link,
        season: 1, number: number, size: nil, progress: progress, resumeAt: progress.map { $0 * 1000 },
        watched: watched, offline: false, providerName: nil, providerOverview: nil, stillUrl: nil,
        airDate: nil, lastPlayed: lastPlayed
    )
}

private func title(_ key: String, _ episodes: [EpisodeRecord]) -> TitleRecord {
    TitleRecord(
        key: key, name: key, year: nil, kind: .series, watchedCount: 0,
        episodeCount: UInt64(episodes.count), canonicalName: nil, originalName: nil, overview: nil,
        metadataProvider: nil, metadataId: nil, metadataYear: nil, metadataKind: nil, posterUrl: nil,
        backdropUrl: nil, rating: nil, genres: [], providerEpisodeCount: nil, externalUrl: nil,
        manualMatch: false, seasons: [SeasonRecord(number: 1, label: "Season 1", episodes: episodes)],
        displayName: key, wideUrl: nil, formatLabel: nil, seasonLabel: nil, studios: [],
        directors: [], tags: [], airing: false, nextEpisode: nil, nextAiringAt: nil,
        franchise: []
    )
}

final class IncomingShareLinkTests: XCTestCase {
    private let link = "https://drive.proton.me/urls/ABC123#s3cr3t"

    func testTheSchemeSwappedLinkKeepsItsFragment() {
        XCTAssertEqual(shareLink(from: URL(string: "protonstream://drive.proton.me/urls/ABC123#s3cr3t")!), link)
    }

    func testTheAddFormCarriesTheWholeLink() {
        let encoded = link.addingPercentEncoding(withAllowedCharacters: .alphanumerics)!
        XCTAssertEqual(shareLink(from: URL(string: "protonstream://add?url=\(encoded)")!), link)
    }

    func testAnythingButAShareLinkIsIgnored() {
        XCTAssertNil(shareLink(from: URL(string: "https://drive.proton.me/urls/ABC123#s3cr3t")!))
        XCTAssertNil(shareLink(from: URL(string: "protonstream://example.com/urls/ABC")!))
        XCTAssertNil(shareLink(from: URL(string: "protonstream://add?url=http%3A%2F%2Fdrive.proton.me%2Furls%2FA")!))
    }

    func testALinkIsFoundInsideTextAndStopsAtItsQuotes() {
        XCTAssertEqual(shareLink(in: "Watch this: \"\(link)\" — enjoy"), link)
        XCTAssertNil(shareLink(in: "no link here"))
    }
}

final class AccountUiTests: XCTestCase {
    func testTheSyncLineSaysHowLongAgoAndWhatCameIn() {
        let zero = Date(timeIntervalSince1970: 0)
        XCTAssertEqual(syncLine(AccountUiState(), now: zero), "Watch history syncs with your Drive")
        XCTAssertEqual(syncLine(AccountUiState(syncedAt: zero), now: zero.addingTimeInterval(30)), "Synced just now")
        XCTAssertEqual(syncLine(AccountUiState(syncedAt: zero), now: zero.addingTimeInterval(300)), "Synced 5 min ago")
        XCTAssertEqual(
            syncLine(AccountUiState(syncedAt: zero, applied: 3), now: zero.addingTimeInterval(7200)),
            "Synced 2 h ago · 3 from other devices"
        )
    }

    func testAFailedSyncIsSaidInPlaceOfWhenItLastWorked() {
        let zero = Date(timeIntervalSince1970: 0)
        XCTAssertEqual(
            syncLine(AccountUiState(syncedAt: zero, syncError: "offline"), now: zero),
            "Watch history did not sync: offline"
        )
    }

    func testTheVerificationPageMayOnlyNavigateWithinProtonOverHttps() {
        XCTAssertTrue(isProtonPage(scheme: "https", host: "verify.proton.me"))
        XCTAssertTrue(isProtonPage(scheme: "https", host: "proton.me"))
        XCTAssertFalse(isProtonPage(scheme: "http", host: "verify.proton.me"))
        XCTAssertFalse(isProtonPage(scheme: "https", host: "proton.me.example.com"))
        XCTAssertFalse(isProtonPage(scheme: "https", host: "notproton.me"))
        XCTAssertFalse(isProtonPage(scheme: "https", host: nil))
    }
}

final class ArtworkTests: XCTestCase {
    func testInitialsSkipTheLowerCaseParticlesOfARomanisedName() {
        XCTAssertEqual(initials("Oshi no Ko"), "OK")
        XCTAssertEqual(initials("Ano Hi Mita Hana no Namae o Bokutachi wa Mada Shiranai."), "AH")
    }

    func testAOneWordNameHasOneInitial() {
        XCTAssertEqual(initials("Akira"), "A")
    }

    func testANameWithNoCapitalsFallsBackToItsFirstTwoWords() {
        XCTAssertEqual(initials("blue world order"), "BW")
    }

    func testPunctuationAndSeparatorsAreNotWords() {
        XCTAssertEqual(initials("Steins;Gate - 2"), "S2")
        XCTAssertEqual(initials(" - "), "")
    }

    /// The placeholder's colour comes from the name's hash, so a title is the
    /// same colour here as on Android only if the hash is the JVM's.
    func testTheNameHashIsTheJvmStringHash() {
        XCTAssertEqual(javaHash(""), 0)
        XCTAssertEqual(javaHash("Akira"), 63_321_038)
        XCTAssertEqual(javaHash("Oshi no Ko"), 539_783_784)
    }
}

final class HistoryTests: XCTestCase {
    func testHistoryIsEveryStartedOrFinishedEpisodeNewestFirstAcrossTitles() {
        let titles = [
            title("a", [
                episode("a1", watched: true, progress: 1, lastPlayed: 30),
                // Marked unwatched again: a row at zero, which is not a play.
                episode("a2", progress: 0, lastPlayed: 40),
                episode("a3"),
            ]),
            title("b", [episode("b1", progress: 0.4, lastPlayed: 50)]),
        ]
        let played = history(titles)
        XCTAssertEqual(played.map(\.episode.linkId), ["b1", "a1"])
        XCTAssertEqual(played.last?.index, 0)
    }

    func testDaysAreHeadedRelativeToTodayThenByDate() {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC")!
        let english = Locale(identifier: "en_GB")
        func day(_ year: Int, _ month: Int, _ day: Int) -> Date {
            calendar.date(from: DateComponents(year: year, month: month, day: day))!
        }
        let today = day(2026, 10, 3) // a Saturday
        XCTAssertEqual(dayHeading(today, today: today, calendar: calendar, locale: english), "Today")
        XCTAssertEqual(dayHeading(day(2026, 10, 2), today: today, calendar: calendar, locale: english), "Yesterday")
        XCTAssertEqual(dayHeading(day(2026, 9, 28), today: today, calendar: calendar, locale: english), "Monday")
        XCTAssertEqual(dayHeading(day(2026, 9, 26), today: today, calendar: calendar, locale: english), "26 September")
        XCTAssertEqual(dayHeading(day(2025, 10, 3), today: today, calendar: calendar, locale: english), "3 October 2025")
    }
}

final class CommonTests: XCTestCase {
    func testPlayLandsOnTheLastPartWatchedEpisodeThenTheFirstUnwatched() {
        XCTAssertEqual(nextUpIndex([episode("1", watched: true), episode("2", progress: 0.5, lastPlayed: 10), episode("3", progress: 0.2, lastPlayed: 20)]), 2)
        XCTAssertEqual(nextUpIndex([episode("1", watched: true), episode("2"), episode("3")]), 1)
        XCTAssertEqual(nextUpIndex([episode("1", watched: true)]), 0)
        XCTAssertEqual(nextUpIndex([]), 0)
    }

    func testBytesAreWrittenInBinaryUnitsToOnePlace() {
        XCTAssertEqual(formatBytes(512), "512 bytes")
        XCTAssertEqual(formatBytes(1536), "1.5 KiB")
        XCTAssertEqual(formatBytes(4 * 1024 * 1024), "4.0 MiB")
        XCTAssertEqual(formatBytes(3 * 1024 * 1024 * 1024 / 2), "1.5 GiB")
    }

    func testAnEpisodeIsHeadedByItsNameAndNumberBeforeItsFilename() {
        var named = episode("e", number: 3)
        named.providerName = "Mother and Children"
        XCTAssertEqual(named.heading(0), "3. Mother and Children")
        XCTAssertEqual(episode("e", number: 7).heading(0), "7. e")
        var bare = episode("e")
        bare.detail = bare.name
        XCTAssertEqual(bare.heading(0), "e")
        bare.number = nil
        bare.name = ".mkv"
        bare.detail = bare.name
        XCTAssertEqual(bare.heading(2), "Episode 3")
        bare.name = "e.mkv"
        bare.detail = bare.name
        bare.number = 4
        XCTAssertEqual(bare.heading(0), "Episode 4")
    }

    func testEpisodesAreNumberedBySeasonThenByPlaceInTheList() {
        var numbered = episode("e", number: 38)
        numbered.season = 3
        XCTAssertEqual(numbered.numbering(nil, 0), "S03E38")
        numbered.season = nil
        XCTAssertEqual(numbered.numbering(nil, 0), "E38")
        XCTAssertEqual(numbered.numbering(2, 0), "S02E38")
        numbered.number = nil
        XCTAssertEqual(numbered.numbering(nil, 3), "#4")
    }

    func testClockReadsMinutesThenHours() {
        XCTAssertEqual(clock(0), "0:00")
        XCTAssertEqual(clock(724), "12:04")
        XCTAssertEqual(clock(3725), "1:02:05")
    }
}

final class LanguagePickerTests: XCTestCase {
    func testAStoredTagReadsAsItsLanguageInEitherIsoForm() {
        XCTAssertEqual(languageLabel("jpn"), "Japanese")
        XCTAssertEqual(languageLabel("ja"), "Japanese")
        XCTAssertEqual(languageLabel("deu"), "German")
        XCTAssertEqual(languageLabel("ger"), "German")
    }

    func testARegionSuffixAndCapitalsDoNotHideTheLanguage() {
        XCTAssertEqual(languageLabel("PT-br"), "Portuguese")
    }

    func testATagNotOnTheListIsShownAsTypedAndBlankIsNoPreference() {
        XCTAssertEqual(languageLabel("tgl"), "tgl")
        XCTAssertEqual(languageLabel(nil), "No preference")
        XCTAssertEqual(languageLabel("  "), "No preference")
    }

    func testEveryListedLanguageHasATwoLetterForm() {
        for language in languages {
            XCTAssertEqual(languageLabel(twoLetter[language.code]), language.name, language.code)
        }
    }
}

final class ThemeTests: XCTestCase {
    /// The same mapping as Android's `schemeOf`: a role that drifted to a
    /// different palette colour on one client is the two clients disagreeing.
    func testEveryRoleIsTheSamePaletteColourAndroidUses() {
        let palette = protonFallback
        let scheme = Scheme(palette)
        XCTAssertEqual(scheme.primary, argb(palette.accent))
        XCTAssertEqual(scheme.onPrimary, argb(palette.onAccent))
        XCTAssertEqual(scheme.secondary, argb(palette.accentAlt))
        XCTAssertEqual(scheme.secondaryContainer, argb(palette.accentDim))
        XCTAssertEqual(scheme.tertiary, argb(palette.accentAlt))
        XCTAssertEqual(scheme.background, argb(palette.background))
        XCTAssertEqual(scheme.surface, argb(palette.surface))
        XCTAssertEqual(scheme.onSurface, argb(palette.text))
        XCTAssertEqual(scheme.onSurfaceVariant, argb(palette.muted))
        XCTAssertEqual(scheme.inverseSurface, argb(palette.text))
        XCTAssertEqual(scheme.inverseOnSurface, argb(palette.background))
        XCTAssertEqual(scheme.error, argb(palette.danger))
        XCTAssertEqual(scheme.errorContainer, argb(palette.dangerDim))
        XCTAssertEqual(scheme.outline, argb(palette.muted))
        XCTAssertEqual(scheme.outlineVariant, argb(palette.border))
        XCTAssertEqual(scheme.surfaceContainerLowest, argb(palette.sunken))
        XCTAssertEqual(scheme.surfaceContainer, argb(palette.card))
        XCTAssertEqual(scheme.surfaceContainerHigh, argb(palette.cardHover))
        XCTAssertEqual(scheme.surfaceContainerHighest, argb(palette.elevated))
        XCTAssertFalse(scheme.light)
    }

    func testArgbUnpacksEveryChannel() {
        let colour = UIColor(argb(0x80FF_4000))
        var red: CGFloat = 0, green: CGFloat = 0, blue: CGFloat = 0, alpha: CGFloat = 0
        colour.getRed(&red, green: &green, blue: &blue, alpha: &alpha)
        XCTAssertEqual(red, 1, accuracy: 0.001)
        XCTAssertEqual(green, 64 / 255, accuracy: 0.001)
        XCTAssertEqual(blue, 0, accuracy: 0.001)
        XCTAssertEqual(alpha, 128 / 255, accuracy: 0.001)
    }

    /// Every style on one of the nine rungs, every rung used, and leading
    /// derived from the size — Android's `TypeRampTest`.
    func testEveryStyleIsOnTheDesktopRamp() {
        let rungs: Set<CGFloat> = [26, 20, 18, 17, 15, 14, 13, 12, 11]
        let used = Set(TypeRole.allCases.map(\.size))
        XCTAssertEqual(used, rungs)
    }

    func testOnlyHeadingsAreSemibold() {
        XCTAssertEqual(
            Set(TypeRole.allCases.filter(\.semibold)),
            [.displayLarge, .displayMedium, .displaySmall, .headlineLarge, .headlineMedium, .headlineSmall, .titleLarge]
        )
    }
}
