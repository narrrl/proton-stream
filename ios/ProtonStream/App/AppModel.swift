import Foundation
import Observation
import UIKit

/// The Proton account: where a sign-in stands, and how sync is doing.
struct AccountUiState: Equatable {
    /// Nil until the bridge has said.
    var state: AccountState?
    /// A sign-in step is out and not answered yet.
    var busy = false
    /// Why the last sign-in step was refused.
    var error: String?
    /// The CAPTCHA page Proton wants solved before it accepts the sign-in.
    var verificationUrl: String?
    /// When watch history last synced.
    var syncedAt: Date?
    /// Positions the last sync took from other devices.
    var applied = 0
    /// Why the last sync failed, until one succeeds.
    var syncError: String?
}

/// One episode's watch state as it was before a change the viewer can undo.
/// Nil `state` means it had never been played.
struct WatchSnapshot: Equatable {
    let shareId: String
    let linkId: String
    let state: WatchStateRecord?
}

/// The app's screen state and every action on it: Android's `AppViewModel`,
/// field for field and message for message.
///
/// Rust owns the Proton sessions, SQLite and every decrypted byte; this asks it
/// questions off the main thread and keeps the answers for SwiftUI.
@MainActor
@Observable
final class AppModel {
    var loading = true
    var refreshing = false
    private(set) var query = ""
    var titles: [TitleRecord] = []
    /// The grid: `titles` ordered, each franchise folded into one tile when
    /// `grouped`, and the shelves above it. Nil until the bridge has answered,
    /// when the grid falls back to `titles` as they are.
    var arrangement: ArrangementRecord?
    var sort: LibrarySort = .name
    var grouped = true
    var shares: [ShareRecord] = []
    var offline: [OfflineRecord] = []
    var metadataSettings = MetadataSettingsRecord(enabled: false, provider: .aniList, language: "en", ready: true)
    var storage = StorageUsageRecord(offlineBytes: 0, offlineCount: 0, partialBytes: 0, cacheBytes: 0)
    /// What the snackbar says.
    var message: String?
    /// What is running in the background right now, said in a banner for as
    /// long as it runs: an add, a crawl, a match, a sync. Without it a
    /// minute-long crawl looked like a tap that did nothing.
    private(set) var activity: String?
    /// What Undo on `message` puts back, when the message offers one.
    var undo: [WatchSnapshot]?
    var account = AccountUiState()

    @ObservationIgnored private var searchTask: Task<Void, Never>?
    @ObservationIgnored private var started = false
    /// Launch and the first `.active` both ask; one crawl answers.
    @ObservationIgnored private var catchingUp = false

    /// Five minutes, as on Android: cheap when nothing changed — one folder
    /// listing, and no upload.
    private static let syncInterval: Duration = .seconds(5 * 60)

    func start() {
        if started { return }
        started = true
        reload()
        enteredForeground()
        DownloadCoordinator.shared.onFinished = { [weak self] in self?.reload() }
        DownloadCoordinator.shared.resumeQueued()
        Task {
            let state = (try? await run { $0.accountState() }) ?? .signedOut
            account.state = state
            // While the app is alive, on top of after sign-in and whenever the
            // player closes.
            while !Task.isCancelled {
                await syncWatchHistory()
                try? await Task.sleep(for: Self.syncInterval)
            }
        }
    }

    /// Run a blocking bridge call off the main thread. The synchronous exports
    /// take the catalog lock and touch SQLite, which a scroll must not wait on.
    nonisolated func run<T: Sendable>(_ work: @escaping @Sendable (AndroidEngine) throws -> T) async throws -> T {
        try await Task.detached(priority: .userInitiated) {
            try work(try NativeRuntime.blockingEngine())
        }.value
    }

    // MARK: - Library

    /// Reorder the grid, or fold or unfold its franchises.
    func arrange(_ sort: LibrarySort, _ grouped: Bool) {
        self.sort = sort
        self.grouped = grouped
        Task { await reloadLibrary(query) }
    }

    func search(_ query: String) {
        self.query = query
        searchTask?.cancel()
        searchTask = Task {
            try? await Task.sleep(for: .milliseconds(300))
            if Task.isCancelled { return }
            await reloadLibrary(query)
        }
    }

    func refresh() {
        crawl(nil)
    }

    /// Recrawl one share rather than the whole library: a share whose link has
    /// expired makes a whole-library refresh fail, and must not leave no way to
    /// refresh the shares that still work.
    func refreshShare(_ id: String) {
        crawl(id)
    }

    private func crawl(_ shareId: String?) {
        Task { await crawlNow(shareId) }
    }

    /// The crawl itself, for pull to refresh, which holds its spinner until
    /// this returns.
    func crawlNow(_ shareId: String?) async {
        let name = shareId.flatMap { id in shares.first { $0.id == id }?.name }
        await busy(name.map { "Reading \($0)…" } ?? "Refreshing the library…") {
            message = nil
            do {
                try await NativeRuntime.engine().crawl(shareId: shareId)
            } catch {
                message = errorMessage(error)
            }
        }
        reload()
    }

    /// Run enrichment again. With `force` every title is looked up afresh,
    /// including ones that already matched.
    func matchTitles(force: Bool) {
        Task { await match(force: force) }
    }

    private func match(force: Bool) async {
        await busy("Looking up titles…") {
            do {
                let summary = try await NativeRuntime.engine().matchTitles(force: force)
                message = Self.describe(summary)
            } catch {
                message = errorMessage(error)
            }
        }
        reload()
    }

    /// Say `what` in the activity banner while `work` runs. Nested work keeps
    /// the outer line once the inner one is done.
    private func busy(_ what: String, _ work: () async -> Void) async {
        let outer = activity
        activity = what
        refreshing = true
        await work()
        activity = outer
        refreshing = outer != nil
    }

    // MARK: - Shares

    /// Add a link, and return why it was refused, if it was.
    ///
    /// The link is opened before anything is stored, so the form can stay up
    /// with the reason under it and a retry is a retry — not "already in the
    /// library" for a share that never opened. Only the new share is crawled:
    /// another share that fails to open must not make this one look broken.
    func addShare(name: String, url: String, password: String?) async -> String? {
        let password = password.flatMap { $0.trimmingCharacters(in: .whitespaces).isEmpty ? nil : $0 }
        let share: ShareRecord
        do {
            share = try await NativeRuntime.engine().addShare(name: name, url: url, customPassword: password)
        } catch {
            return errorMessage(error)
        }
        reload()
        Task { await crawlAdded([share.id], announce: true) }
        return nil
    }

    /// Crawl shares that are new here — added on this device or by sync —
    /// then match them when enrichment is on.
    private func crawlAdded(_ ids: [String], announce: Bool) async {
        guard !ids.isEmpty else { return }
        let before = Set(titles.map(\.key))
        var failed: [String] = []
        for id in ids {
            let name = shares.first { $0.id == id }?.name ?? "the new share"
            await busy("Reading \(name)…") {
                do {
                    try await NativeRuntime.engine().crawl(shareId: id)
                } catch {
                    failed.append("\(name): \(errorMessage(error))")
                }
            }
        }
        await reloadNow()
        if !failed.isEmpty {
            message = failed.joined(separator: "\n")
            return
        }
        if metadataSettings.enabled {
            await match(force: false)
        }
        if announce {
            let added = Set(titles.map(\.key)).subtracting(before).count
            message = added == 1 ? "1 title added" : "\(added) titles added"
        }
    }

    /// Re-supply the link behind a share whose stored secret cannot be read.
    /// Re-entering the link rewrites the secret and leaves the catalog and the
    /// offline files alone.
    func repairShare(id: String, url: String, password: String?) {
        Task {
            do {
                let password = password.flatMap { $0.trimmingCharacters(in: .whitespaces).isEmpty ? nil : $0 }
                _ = try await run { try $0.repairShare(shareId: id, url: url, customPassword: password) }
                try await NativeRuntime.engine().crawl(shareId: id)
            } catch {
                reportError(error)
            }
            reload()
        }
    }

    func removeShare(_ id: String) {
        Task {
            do {
                // Wait for the transfers to stop before Rust removes catalog
                // and files. Rust also rejects any late publication.
                await DownloadCoordinator.shared.cancelShare(id)
                let store = DownloadStateStore.shared
                let engine = try await NativeRuntime.engine()
                for download in store.records() where download.shareId == id {
                    try await engine.removeOfflineEpisode(shareId: download.shareId, linkId: download.linkId)
                }
                store.removeShare(id)
                try await run { try $0.removeShare(shareId: id) }
            } catch {
                message = errorMessage(error)
            }
            reload()
        }
    }

    func dismissMessage() {
        message = nil
        undo = nil
    }

    // MARK: - Account

    /// Start signing in, or start again with `verificationToken` once the
    /// CAPTCHA from a first try is solved.
    func signIn(_ username: String, _ password: String, verificationToken: String? = nil) {
        signInStep { try await $0.signIn(username: username, password: password, verificationToken: verificationToken) }
    }

    func submitSecondFactor(_ code: String) {
        signInStep { try await $0.submitSecondFactor(code: code) }
    }

    func submitMailboxPassword(_ password: String) {
        signInStep(keepStepOnFailure: true) { try await $0.submitMailboxPassword(password: password) }
    }

    func cancelSignIn() {
        account = AccountUiState(state: .signedOut)
        Task { try? await run { $0.cancelSignIn() } }
    }

    /// The CAPTCHA page closed without a token.
    func dismissVerification() {
        account.verificationUrl = nil
        account.busy = false
    }

    private func signInStep(keepStepOnFailure: Bool = false, _ step: @escaping (AndroidEngine) async throws -> SignInOutcome) {
        account.busy = true
        account.error = nil
        account.verificationUrl = nil
        Task {
            do {
                let outcome = try await step(NativeRuntime.engine())
                switch outcome {
                case let .done(username):
                    account = AccountUiState(state: .signedIn(username: username))
                    message = "Signed in as \(username)"
                    await syncWatchHistory()
                    reload()
                case .secondFactor:
                    account.state = .secondFactor
                    account.busy = false
                case .mailboxPassword:
                    account.state = .mailboxPassword
                    account.busy = false
                case let .humanVerification(url):
                    account.verificationUrl = url
                }
            } catch {
                account.busy = false
                account.error = errorMessage(error)
                // A refused code spends the half-made session.
                if !keepStepOnFailure { account.state = .signedOut }
            }
        }
    }

    func signOut() {
        Task {
            do {
                try await NativeRuntime.engine().signOut()
            } catch {
                reportError(error)
            }
            account = AccountUiState(state: .signedOut)
            reload()
        }
    }

    /// Sync now, from a tap: says so when it is done.
    func syncNow() {
        Task {
            var synced = false
            await busy("Syncing with your other devices…") { synced = await syncWatchHistory() }
            guard synced else { return }
            let applied = account.applied
            message = applied > 0 ? "Synced, \(applied) positions from other devices" : "Everything is up to date"
        }
    }

    /// Pull other devices' positions, shares and settings in and push this
    /// one's out. Quiet: it runs on a timer, and a failure is shown on the
    /// account card rather than as a snackbar every five minutes of an offline
    /// evening. Returns whether it synced.
    @discardableResult
    func syncWatchHistory() async -> Bool {
        guard case .signedIn = account.state else { return false }
        do {
            guard let report = try await NativeRuntime.engine().syncWatchHistory() else { return false }
            account.syncedAt = Date()
            account.applied = Int(report.applied)
            account.syncError = nil
            await takeSynced(report)
            return true
        } catch {
            let state = try? await run { $0.accountState() }
            if let state { account.state = state }
            account.syncError = errorMessage(error)
            // Signed out by the engine: the session ended, and that is worth
            // interrupting for, unlike a dropped connection.
            if state == .signedOut { reportError(error) }
            return false
        }
    }

    /// What another device changed: its shares crawled or dropped here, its
    /// settings repainted.
    private func takeSynced(_ report: SyncRecord) async {
        for id in report.sharesRemoved {
            await DownloadCoordinator.shared.cancelShare(id)
            DownloadStateStore.shared.removeShare(id)
        }
        if report.settingsChanged {
            await AppearanceState.shared.reload()
        }
        if report.applied > 0 || report.titles > 0 || report.settingsChanged || !report.sharesRemoved.isEmpty {
            await reloadNow()
        }
        // A crawl takes longer than the grace iOS gives a backgrounded app.
        // What is left uncrawled is picked up on the way back.
        if !report.sharesAdded.isEmpty, UIApplication.shared.applicationState != .background {
            await crawlAdded(report.sharesAdded, announce: false)
            message = report.sharesAdded.count == 1 ? "A share arrived from another device" : "\(report.sharesAdded.count) shares arrived from other devices"
        }
    }

    /// On launch and back on screen: crawl shares the catalog has nothing of —
    /// brought by a sync that ran in the background, or an add whose crawl
    /// never finished.
    func enteredForeground() {
        guard !catchingUp else { return }
        catchingUp = true
        Task {
            defer { catchingUp = false }
            guard let ids = try? await run({ try $0.uncrawledShares() }), !ids.isEmpty else { return }
            await crawlAdded(ids, announce: false)
        }
    }

    /// Leaving the app: push where playback stopped, inside the grace iOS
    /// gives a backgrounded app — what `WatchSyncWorker` does on Android.
    func syncInBackground() {
        guard case .signedIn = account.state else { return }
        let task = BackgroundTask(name: "watch-history-sync")
        Task {
            await syncWatchHistory()
            task.end()
        }
    }

    /// After the player closes: where it stopped is what another device wants next.
    func playerClosed() {
        reload()
        Task { await syncWatchHistory() }
    }

    func addAccountFolder(name: String, volumeId: String, linkId: String) {
        Task {
            do {
                let share = try await run { try $0.addAccountFolder(name: name, volumeId: volumeId, linkId: linkId) }
                reload()
                await crawlAdded([share.id], announce: true)
            } catch {
                reportError(error)
            }
        }
    }

    // MARK: - Watch state

    /// Every episode of a title watched or unwatched, with an Undo: a whole
    /// title's positions are what this throws away, and a mis-tap on a tile's
    /// menu should not cost them.
    func setTitleWatched(_ title: TitleRecord, _ watched: Bool) {
        changeWatch(title.playlist, "\(title.displayName) marked \(watched ? "watched" : "unwatched")") { engine, episode, before in
            let duration = before?.durationSecs
            try engine.saveWatchState(
                shareId: episode.shareId,
                linkId: episode.linkId,
                positionSecs: watched ? duration ?? 0 : 0,
                durationSecs: duration,
                watched: watched
            )
        }
    }

    /// Off Continue watching by forgetting where the episode stopped — what the
    /// desktop does — with an Undo that puts the position back.
    func forgetPosition(_ title: TitleRecord, _ episode: EpisodeRecord) {
        changeWatch([episode], "\(title.displayName) removed from Continue watching") { engine, target, before in
            try engine.saveWatchState(shareId: target.shareId, linkId: target.linkId, positionSecs: 0, durationSecs: before?.durationSecs, watched: false)
        }
    }

    /// Off the history page: the position and the watched mark both go, which
    /// is "never played", with an Undo.
    func removeFromHistory(_ title: TitleRecord, _ episode: EpisodeRecord) {
        changeWatch([episode], "\(episode.label) of \(title.displayName) removed from history") { engine, target, before in
            try engine.saveWatchState(shareId: target.shareId, linkId: target.linkId, positionSecs: 0, durationSecs: before?.durationSecs, watched: false)
        }
    }

    /// Put back what the last undoable change replaced.
    func performUndo() {
        guard let snapshots = undo else { return }
        message = nil
        undo = nil
        Task {
            do {
                try await run { engine in
                    for snapshot in snapshots {
                        try engine.saveWatchState(
                            shareId: snapshot.shareId,
                            linkId: snapshot.linkId,
                            positionSecs: snapshot.state?.positionSecs ?? 0,
                            durationSecs: snapshot.state?.durationSecs,
                            watched: snapshot.state?.watched ?? false
                        )
                    }
                }
            } catch {
                reportError(error)
            }
            reload()
        }
    }

    private func changeWatch(
        _ episodes: [EpisodeRecord],
        _ text: String,
        _ change: @escaping @Sendable (AndroidEngine, EpisodeRecord, WatchStateRecord?) throws -> Void
    ) {
        Task {
            do {
                let snapshots = try await run { engine in
                    try episodes.map { episode in
                        let before = try engine.watchState(shareId: episode.shareId, linkId: episode.linkId)
                        try change(engine, episode, before)
                        return WatchSnapshot(shareId: episode.shareId, linkId: episode.linkId, state: before)
                    }
                }
                message = text
                undo = snapshots
            } catch {
                reportError(error)
            }
            reload()
        }
    }

    func reportError(_ error: Error) {
        message = errorMessage(error)
    }

    func report(_ text: String) {
        message = text
    }

    // MARK: - Metadata

    /// Store the enrichment settings, and return why not, if they were not.
    ///
    /// The page shows the new settings as soon as they are stored; the match
    /// run that follows can take minutes and says so in the activity banner.
    /// Before, the page only changed once that run had finished, so a save
    /// looked like it had done nothing.
    func saveMetadataSettings(enabled: Bool, provider: MetadataProvider, language: String, apiKey: String) async -> String? {
        do {
            metadataSettings = try await run { engine in
                if provider == .tmdb, !apiKey.trimmingCharacters(in: .whitespaces).isEmpty {
                    try engine.setMetadataApiKey(provider: provider, key: apiKey)
                }
                try engine.setMetadataSettings(settings: MetadataSettingsRecord(enabled: enabled, provider: provider, language: language, ready: true))
                return try engine.metadataSettings()
            }
        } catch {
            return errorMessage(error)
        }
        await reloadNow()
        if enabled {
            Task { await match(force: false) }
        }
        return nil
    }

    func reloadAfterMetadataChange() { reload() }

    /// What an enrichment pass did, in one line. Failures are reported even
    /// when most titles matched.
    static func describe(_ summary: MatchSummary) -> String {
        if summary.matched == 0, summary.unmatched == 0, summary.failed == 0 {
            return "Everything is already matched"
        }
        var parts = ["\(summary.matched) matched"]
        if summary.unmatched > 0 { parts.append("\(summary.unmatched) not found") }
        if summary.episodes > 0 { parts.append("\(summary.episodes) episodes named") }
        if summary.failed > 0 { parts.append("\(summary.failed) failed") }
        return parts.joined(separator: ", ")
    }

    // MARK: - Offline

    func removeOffline(_ file: OfflineRecord) {
        Task {
            do {
                try await NativeRuntime.engine().removeOfflineEpisode(shareId: file.shareId, linkId: file.linkId)
            } catch {
                message = errorMessage(error)
            }
            reload()
        }
    }

    func pauseDownload(_ download: RetainedDownload) { DownloadCoordinator.shared.pause(download) }

    func resumeDownload(_ download: RetainedDownload) { DownloadCoordinator.shared.resume(download) }

    func deletePartial(_ download: RetainedDownload) {
        Task {
            await DownloadCoordinator.shared.cancelAndWait(download)
            do {
                try await NativeRuntime.engine().removeOfflineEpisode(shareId: download.shareId, linkId: download.linkId)
                DownloadStateStore.shared.remove(download.shareId, download.linkId)
            } catch {
                reportError(error)
            }
            reload()
        }
    }

    func removeAllOffline() {
        Task {
            await DownloadCoordinator.shared.cancelAll()
            do {
                try await NativeRuntime.engine().removeAllOffline()
                DownloadStateStore.shared.clear()
            } catch {
                reportError(error)
            }
            reload()
        }
    }

    func clearBlockCache() {
        Task {
            do {
                let reclaimed = try await run { try $0.clearBlockCache() }
                message = "Reclaimed \(formatBytes(reclaimed)) of cache"
            } catch {
                reportError(error)
            }
            reload()
        }
    }

    // MARK: - Progress

    /// Persist where an episode was left. Runs on its own task rather than the
    /// player's, so a save issued on the way out is not cancelled by the
    /// player going away.
    func saveProgress(_ episode: EpisodeRecord, position: Double, duration: Double, watched: Bool) {
        Task {
            do {
                try await run { engine in
                    try engine.saveWatchState(
                        shareId: episode.shareId,
                        linkId: episode.linkId,
                        positionSecs: min(max(position, 0), duration),
                        durationSecs: duration,
                        watched: watched
                    )
                }
            } catch {
                reportError(error)
            }
        }
    }

    /// Mark an episode seen, or unseen, without playing it.
    ///
    /// Unwatching rewinds, matching the desktop client. Marking one seen puts
    /// the position at its duration where one is known, so the progress bar
    /// agrees with the tick.
    func setWatched(_ episode: EpisodeRecord, _ watched: Bool) {
        Task {
            do {
                try await run { engine in
                    let duration = try engine.watchState(shareId: episode.shareId, linkId: episode.linkId)?.durationSecs
                    try engine.saveWatchState(
                        shareId: episode.shareId,
                        linkId: episode.linkId,
                        positionSecs: watched ? duration ?? 0 : 0,
                        durationSecs: duration,
                        watched: watched
                    )
                }
            } catch {
                reportError(error)
            }
            reload()
        }
    }

    // MARK: - Reload

    func reload() {
        Task { await reloadNow() }
    }

    /// `reload`, for a caller that goes on to read what it loaded.
    func reloadNow() async {
        let query = query.trimmingCharacters(in: .whitespaces).isEmpty ? nil : self.query
        let sort = sort
        let grouped = grouped
        do {
            let result = try await run { engine in
                // Here rather than inside `library()`, which is read-only
                // and cached: a full reload is the one place that has
                // already paid for a walk of the offline files.
                _ = try engine.pruneOfflineFiles()
                return Reloaded(
                    shares: try engine.shares(),
                    titles: try engine.library(search: query),
                    arrangement: try engine.arrangement(search: query, sort: sort, grouped: grouped),
                    offline: try engine.offlineFiles(),
                    metadataSettings: try engine.metadataSettings(),
                    storage: try engine.storageUsage()
                )
            }
            loading = false
            shares = result.shares
            titles = result.titles
            arrangement = result.arrangement
            offline = result.offline
            metadataSettings = result.metadataSettings
            storage = result.storage
        } catch {
            loading = false
            message = errorMessage(error)
        }
    }

    private func reloadLibrary(_ query: String) async {
        let search = query.trimmingCharacters(in: .whitespaces).isEmpty ? nil : query
        let sort = sort
        let grouped = grouped
        do {
            let (titles, arrangement) = try await run { engine in
                (try engine.library(search: search), try engine.arrangement(search: search, sort: sort, grouped: grouped))
            }
            self.titles = titles
            self.arrangement = arrangement
        } catch {
            message = errorMessage(error)
        }
    }
}

/// The grace iOS gives an app leaving the screen, ended exactly once —
/// whether by the work finishing or by the time running out.
@MainActor
final class BackgroundTask {
    private var identifier = UIBackgroundTaskIdentifier.invalid

    init(name: String) {
        identifier = UIApplication.shared.beginBackgroundTask(withName: name) { [weak self] in
            MainActor.assumeIsolated { self?.end() }
        }
    }

    func end() {
        guard identifier != .invalid else { return }
        UIApplication.shared.endBackgroundTask(identifier)
        identifier = .invalid
    }
}

private struct Reloaded: @unchecked Sendable {
    let shares: [ShareRecord]
    let titles: [TitleRecord]
    let arrangement: ArrangementRecord
    let offline: [OfflineRecord]
    let metadataSettings: MetadataSettingsRecord
    let storage: StorageUsageRecord
}
