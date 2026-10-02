import Foundation
import Observation

/// The app's one Rust engine and the screen state read from it.
///
/// Rust owns the Proton sessions, SQLite and every decrypted byte; this only
/// asks it questions off the main thread and keeps the answers for SwiftUI.
@MainActor
@Observable
final class AppModel {
    let engine: AndroidEngine?
    private(set) var startupError: String?

    var titles: [TitleRecord] = []
    var shares: [ShareRecord] = []
    var search = ""
    var crawling = false
    var error: String?

    init() {
        do {
            engine = try AndroidEngine(paths: Self.paths(), secrets: KeychainSecretStore())
        } catch {
            engine = nil
            startupError = String(describing: error)
        }
    }

    /// Application Support survives updates and is backed up, which is what
    /// the watch history wants. The block cache goes under Caches, which iOS
    /// may purge and never backs up.
    private static func paths() throws -> AndroidPaths {
        let files = FileManager.default
        let support = try files.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
        let caches = try files.url(for: .cachesDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
        let config = support.appendingPathComponent("config", isDirectory: true)
        let data = support.appendingPathComponent("data", isDirectory: true)
        let cache = caches.appendingPathComponent("proton-stream", isDirectory: true)
        for directory in [config, data, cache] {
            try files.createDirectory(at: directory, withIntermediateDirectories: true)
        }
        return AndroidPaths(config: config.path, data: data.path, cache: cache.path)
    }

    struct NoEngine: Error, CustomStringConvertible {
        var description: String { "the Rust engine did not start" }
    }

    /// Run a blocking bridge call off the main thread. The synchronous exports
    /// take the catalog lock and touch SQLite, which a scroll must not wait on.
    func run<T>(_ work: @escaping (AndroidEngine) throws -> T) async throws -> T {
        guard let engine else { throw NoEngine() }
        return try await Task.detached(priority: .userInitiated) { try work(engine) }.value
    }

    func report(_ error: Error) {
        self.error = String(describing: error)
    }

    func reload() async {
        let needle = search.trimmingCharacters(in: .whitespaces)
        do {
            let (titles, shares) = try await run { engine in
                (try engine.library(search: needle.isEmpty ? nil : needle), try engine.shares())
            }
            self.titles = titles
            self.shares = shares
        } catch {
            report(error)
        }
    }

    func crawl(shareId: String? = nil) async {
        guard let engine, !crawling else { return }
        crawling = true
        defer { crawling = false }
        do {
            try await engine.crawl(shareId: shareId)
        } catch {
            report(error)
        }
        await reload()
    }

    func addShare(name: String, url: String, password: String?) async {
        do {
            let share = try await run { try $0.addShare(name: name, url: url, customPassword: password) }
            await crawl(shareId: share.id)
        } catch {
            report(error)
        }
    }

    func repairShare(id: String, url: String, password: String?) async {
        do {
            _ = try await run { try $0.repairShare(shareId: id, url: url, customPassword: password) }
            await crawl(shareId: id)
        } catch {
            report(error)
        }
    }

    func removeShare(id: String) async {
        do {
            try await run { try $0.removeShare(shareId: id) }
        } catch {
            report(error)
        }
        await reload()
    }

    /// Marking seen puts the position at the duration where one is known, and
    /// unwatching rewinds, as on desktop and Android.
    func setWatched(_ episode: EpisodeRecord, _ watched: Bool) async {
        do {
            try await run { engine in
                let duration = try engine.watchState(shareId: episode.shareId, linkId: episode.linkId)?.durationSecs
                try engine.saveWatchState(
                    shareId: episode.shareId, linkId: episode.linkId,
                    positionSecs: watched ? duration ?? 0 : 0,
                    durationSecs: duration, watched: watched
                )
            }
        } catch {
            report(error)
        }
        await reload()
    }

    /// Episodes started but not finished, most recent first.
    var continueWatching: [(title: TitleRecord, episode: EpisodeRecord)] {
        titles
            .flatMap { title in
                title.seasons.flatMap(\.episodes)
                    .filter { $0.resumeAt != nil && !$0.watched }
                    .map { (title: title, episode: $0) }
            }
            .sorted { $0.episode.lastPlayed > $1.episode.lastPlayed }
    }
}
