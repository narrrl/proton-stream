import Foundation
import Network
import Observation
import UIKit

/// Offline downloads: the queue, the retries and the network policy.
///
/// Rust owns the block-aligned transfer and the catalog mutation, as on
/// Android; this owns what WorkManager owns there. iOS has no equivalent of a
/// foreground worker for a transfer that is not a `URLSession` task, so a
/// download runs while the app does, plus the grace `beginBackgroundTask`
/// grants once it leaves the screen. One cut short is put back in the queue,
/// and the `.part` file resumes at its last whole block on the next launch.
@MainActor
@Observable
final class DownloadCoordinator {
    static let shared = DownloadCoordinator()

    /// Every retained download, sorted by label. Re-read after each write.
    private(set) var records: [RetainedDownload] = []

    /// A download finished or was removed: the catalog has changed.
    @ObservationIgnored var onFinished: (() -> Void)?

    @ObservationIgnored private let store = DownloadStateStore.shared
    @ObservationIgnored private var running: [String: Transfer] = [:]
    @ObservationIgnored private let monitor = NWPathMonitor()
    @ObservationIgnored private var path: NWPath?
    @ObservationIgnored private var refreshQueued = false

    private init() {
        store.onChange = { [weak self] in
            Task { @MainActor in self?.scheduleRefresh() }
        }
        records = store.records()
        monitor.pathUpdateHandler = { [weak self] path in
            Task { @MainActor in self?.pathChanged(path) }
        }
        monitor.start(queue: DispatchQueue(label: "io.narl.protonstream.network"))
    }

    /// Free space to leave behind a download, for the catalog and the caches.
    private static let storageHeadroom: Int64 = 256 * 1024 * 1024
    private static let maxAttempts = 3

    func enqueue(_ episode: EpisodeRecord) { enqueue([episode]) }

    func enqueue(_ episodes: [EpisodeRecord]) {
        let wanted = episodes.filter { !$0.offline }
        if wanted.isEmpty { return }
        // Space is checked once for the whole request, against its total.
        // Queueing a season that cannot fit and discovering it episode by
        // episode at ENOSPC is how the app fills its own database's disk.
        let room = Self.roomFor(wanted.reduce(Int64(0)) { $0 + Int64($1.size ?? 0) })
        let downloads = wanted.map { episode in
            RetainedDownload(
                shareId: episode.shareId,
                volumeId: episode.volumeId,
                linkId: episode.linkId,
                label: episode.label,
                total: Int64(episode.size ?? 0),
                status: room ? RetainedDownload.statusQueued : RetainedDownload.statusFailed,
                error: room ? nil : "Not enough free space"
            )
        }
        // An episode already in flight keeps its record and its transfer.
        store.putAll(downloads.filter { running[$0.key] == nil })
        if room { downloads.forEach(start) }
    }

    func resume(_ download: RetainedDownload) {
        var queued = download
        queued.status = RetainedDownload.statusQueued
        queued.error = nil
        guard let transfer = running[download.key] else {
            store.put(queued)
            start(queued)
            return
        }
        // Still running: nothing to resume. Still winding down from a pause:
        // let it record that first, or its last write lands over this one.
        guard transfer.observer.stopped else { return }
        Task {
            await transfer.task.value
            store.put(queued)
            start(queued)
        }
    }

    func pause(_ download: RetainedDownload) { setStatus(download, RetainedDownload.statusPaused) }

    func cancel(_ download: RetainedDownload) { setStatus(download, RetainedDownload.statusCancelled) }

    /// Stop one download and wait for its transfer to let go of the `.part`
    /// file, so the caller can remove it.
    func cancelAndWait(_ download: RetainedDownload) async {
        cancel(download)
        await running[download.key]?.task.value
    }

    /// Stop every download of one share, and wait for them.
    func cancelShare(_ shareId: String) async {
        let transfers = running.values.filter { $0.download.shareId == shareId }
        for transfer in transfers { stop(transfer.download.key, as: RetainedDownload.statusCancelled) }
        for transfer in transfers { await transfer.task.value }
    }

    /// Stop everything, and wait.
    func cancelAll() async {
        let transfers = Array(running.values)
        for transfer in transfers { stop(transfer.download.key, as: RetainedDownload.statusCancelled) }
        for transfer in transfers { await transfer.task.value }
    }

    /// Start whatever is queued — on launch, back in the foreground, and when
    /// the network comes back. A download the system cut short is queued, not
    /// failed, so it is picked up here.
    func resumeQueued() {
        for download in store.records() where download.status == RetainedDownload.statusQueued
            || download.status == RetainedDownload.statusRunning {
            start(download)
        }
    }

    /// Re-apply "Wi-Fi only" to what is queued and running: a season queued
    /// before the setting was turned on must not then download over cellular.
    func applyNetworkPolicy() {
        if allowed {
            resumeQueued()
        } else {
            for key in Array(running.keys) { stop(key, as: RetainedDownload.statusQueued) }
        }
    }

    // MARK: - Running

    private final class Transfer {
        let download: RetainedDownload
        let observer: Observer
        var task: Task<Void, Never>!

        init(download: RetainedDownload, observer: Observer) {
            self.download = download
            self.observer = observer
        }
    }

    /// Whether the network policy lets a transfer run now.
    private var allowed: Bool {
        guard let path, path.status == .satisfied else { return false }
        return !SettingsStore().wifiOnly || !(path.isExpensive || path.usesInterfaceType(.cellular))
    }

    private func pathChanged(_ path: NWPath) {
        self.path = path
        applyNetworkPolicy()
    }

    private func start(_ download: RetainedDownload) {
        guard running[download.key] == nil, allowed else { return }
        let observer = Observer(store: store, download: download)
        let transfer = Transfer(download: download, observer: observer)
        running[download.key] = transfer
        transfer.task = Task { [weak self] in
            await self?.run(transfer)
        }
    }

    /// Ask a transfer to stop at its next block boundary. `status` is what its
    /// record says afterwards; nil leaves that to whoever called.
    private func stop(_ key: String, as status: String?) {
        guard let transfer = running[key] else { return }
        transfer.observer.stop(as: status)
    }

    private func setStatus(_ download: RetainedDownload, _ status: String) {
        store.update(download.shareId, download.linkId) { stored in
            var changed = stored ?? download
            changed.status = status
            changed.error = nil
            return changed
        }
        stop(download.key, as: status)
    }

    private func run(_ transfer: Transfer) async {
        let download = transfer.download
        let observer = transfer.observer
        let background = UIApplication.shared.beginBackgroundTask(withName: "offline-\(download.linkId)") {
            // Out of background time: stop at the next block and queue it, so
            // the next launch carries on from the `.part` file.
            observer.stop(as: RetainedDownload.statusQueued)
        }
        defer {
            running[download.key] = nil
            UIApplication.shared.endBackgroundTask(background)
        }
        for attempt in 1 ... Self.maxAttempts {
            // A pause or a cancel that landed before this got going.
            if let stored = store.get(download.shareId, download.linkId),
               stored.status == RetainedDownload.statusPaused || stored.status == RetainedDownload.statusCancelled {
                return
            }
            // Queued, not running: the engine lets two downloads transfer at
            // once and holds the rest back, and only the first progress report
            // means this one has a slot.
            store.update(download.shareId, download.linkId) { stored in
                var current = stored ?? download
                current.status = RetainedDownload.statusQueued
                current.error = nil
                current.bytesPerSecond = 0
                return current
            }
            do {
                let engine = try await NativeRuntime.engine()
                _ = try await engine.downloadEpisode(
                    shareId: download.shareId,
                    volumeId: download.volumeId,
                    linkId: download.linkId,
                    observer: observer
                )
                store.remove(download.shareId, download.linkId)
                onFinished?()
                return
            } catch {
                if let status = observer.stoppedAs {
                    // A pause is also a stop. Under the lock, so the record
                    // read here is the one the write lands on.
                    store.update(download.shareId, download.linkId) { stored in
                        guard var current = stored, current.status != RetainedDownload.statusPaused else { return nil }
                        current.status = status
                        current.bytesPerSecond = 0
                        return current
                    }
                    return
                }
                if observer.stopped { return }
                let message = errorMessage(error)
                if attempt < Self.maxAttempts {
                    store.update(download.shareId, download.linkId) { stored in
                        var current = stored ?? download
                        current.status = RetainedDownload.statusQueued
                        current.error = message
                        current.bytesPerSecond = 0
                        return current
                    }
                    // WorkManager's exponential backoff, from 30 s.
                    try? await Task.sleep(for: .seconds(30 * (1 << (attempt - 1))))
                    if observer.stopped { return }
                } else {
                    store.update(download.shareId, download.linkId) { stored in
                        var current = stored ?? download
                        current.status = RetainedDownload.statusFailed
                        current.error = message
                        current.bytesPerSecond = 0
                        return current
                    }
                }
            }
        }
    }

    private func scheduleRefresh() {
        // Progress lands a dozen times a second on a fast link; one re-read
        // per frame is plenty.
        if refreshQueued { return }
        refreshQueued = true
        Task { @MainActor in
            refreshQueued = false
            records = store.records()
        }
    }

    /// Whether `bytes` plus working room will fit where offline files are kept.
    /// An unknown size — zero — is allowed through.
    private static func roomFor(_ bytes: Int64) -> Bool {
        if bytes <= 0 { return true }
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        guard let values = try? support.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey]),
              let available = values.volumeAvailableCapacityForImportantUsage
        else { return true }
        return available - bytes >= storageHeadroom
    }
}

/// The bridge's view of one transfer: where it is, and whether to stop.
private final class Observer: DownloadObserver, @unchecked Sendable {
    private let store: DownloadStateStore
    private let download: RetainedDownload
    private let lock = NSLock()
    private var rate = TransferRate()
    private var reportedAt: Int64 = 0
    private var reportedPercent = -1
    private var stopRequested = false
    private var stopStatus: String?

    /// Shortest gap between two identical-looking progress reports.
    private static let progressIntervalMs: Int64 = 1000

    init(store: DownloadStateStore, download: RetainedDownload) {
        self.store = store
        self.download = download
    }

    func stop(as status: String?) {
        lock.withLock {
            stopRequested = true
            if let status { stopStatus = status }
        }
    }

    var stopped: Bool { lock.withLock { stopRequested } }
    var stoppedAs: String? { lock.withLock { stopStatus } }

    /// Called once per 4 MiB block. Nothing the viewer can see changes that
    /// often, so a report is only made when the percentage moves or a second
    /// has passed. The final one is never suppressed.
    func onProgress(downloaded: UInt64, total: UInt64) throws {
        let percent = total == 0 ? 0 : Int(downloaded * 100 / total)
        let now = Int64(ProcessInfo.processInfo.systemUptime * 1000)
        let complete = total > 0 && downloaded >= total
        let report: Int64? = lock.withLock {
            let bytesPerSecond = rate.sample(Int64(downloaded), now: now)
            if !complete, percent == reportedPercent, now - reportedAt < Self.progressIntervalMs { return nil }
            reportedAt = now
            reportedPercent = percent
            return bytesPerSecond
        }
        guard let bytesPerSecond = report else { return }
        store.update(download.shareId, download.linkId) { [download] stored in
            var current = stored ?? download
            current.downloaded = Int64(downloaded)
            current.total = Int64(total)
            current.bytesPerSecond = bytesPerSecond
            if current.status != RetainedDownload.statusPaused, current.status != RetainedDownload.statusCancelled {
                current.status = RetainedDownload.statusRunning
            }
            return current
        }
    }

    func isCancelled() throws -> Bool { stopped }
}
