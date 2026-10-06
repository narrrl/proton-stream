import Foundation

/// Durable user-facing state for one offline download, kept until it finishes
/// or the viewer deletes it. The fields and their JSON names are Android's
/// `RetainedDownload`.
struct RetainedDownload: Equatable, Hashable, Identifiable, Sendable {
    var shareId: String
    var volumeId: String
    var linkId: String
    var label: String
    var downloaded: Int64 = 0
    var total: Int64 = 0
    var status: String = RetainedDownload.statusQueued
    var error: String?
    /// Recent transfer rate while running; 0 when unknown or not running.
    var bytesPerSecond: Int64 = 0

    var key: String { downloadKey(shareId, linkId) }
    var id: String { key }

    static let statusQueued = "queued"
    static let statusRunning = "running"
    static let statusPaused = "paused"
    static let statusFailed = "failed"
    static let statusCancelled = "cancelled"
}

func downloadKey(_ shareId: String, _ linkId: String) -> String { "\(shareId)\u{1f}\(linkId)" }

/// The retained downloads, as one JSON file of `key → record` beside the
/// catalog — Android keeps the same records in SharedPreferences.
///
/// Every write goes through one lock: the transfer's progress reports and the
/// viewer's pause are concurrent writers to the same record, and a whole-record
/// `put` from a screen's stale copy would both lose that progress and race the
/// transfer's own read-modify-write.
final class DownloadStateStore: @unchecked Sendable {
    static let shared = DownloadStateStore(file: DownloadStateStore.defaultFile)

    /// Called after every write, on whichever thread wrote.
    var onChange: (() -> Void)?

    private let file: URL
    private let lock = NSLock()
    private var entries: [String: String]?

    init(file: URL) {
        self.file = file
    }

    func records() -> [RetainedDownload] {
        lock.withLock { loaded().values.compactMap(Self.decode) }
            .sorted { $0.label.lowercased() < $1.label.lowercased() }
    }

    func get(_ shareId: String, _ linkId: String) -> RetainedDownload? {
        lock.withLock { loaded()[downloadKey(shareId, linkId)].flatMap(Self.decode) }
    }

    func put(_ record: RetainedDownload) {
        edit { $0[record.key] = Self.encode(record) }
    }

    /// Read-modify-write one record, excluding every other writer. Returning
    /// nil from `change` leaves the record alone.
    func update(_ shareId: String, _ linkId: String, _ change: (RetainedDownload?) -> RetainedDownload?) {
        edit { entries in
            let key = downloadKey(shareId, linkId)
            if let changed = change(entries[key].flatMap(Self.decode)) {
                entries[changed.key] = Self.encode(changed)
            }
        }
    }

    /// Write a whole queue in one go: queueing a season is one write, not one
    /// per episode.
    func putAll(_ records: [RetainedDownload]) {
        edit { entries in
            for record in records { entries[record.key] = Self.encode(record) }
        }
    }

    func remove(_ shareId: String, _ linkId: String) {
        edit { $0[downloadKey(shareId, linkId)] = nil }
    }

    /// Forget every retained download. Callers stop the transfers first.
    func clear() {
        edit { $0.removeAll() }
    }

    func removeShare(_ shareId: String) {
        edit { entries in
            entries = entries.filter { Self.decode($0.value)?.shareId != shareId }
        }
    }

    private func edit(_ change: (inout [String: String]) -> Void) {
        lock.withLock {
            var current = loaded()
            change(&current)
            entries = current
            // Atomic, so a kill mid-write leaves the last good file.
            if let data = try? JSONEncoder().encode(current) {
                try? data.write(to: file, options: [.atomic])
            }
        }
        onChange?()
    }

    /// Under `lock`.
    private func loaded() -> [String: String] {
        if let entries { return entries }
        let read = (try? Data(contentsOf: file)).flatMap { try? JSONDecoder().decode([String: String].self, from: $0) } ?? [:]
        entries = read
        return read
    }

    static func encode(_ record: RetainedDownload) -> String {
        var json: [String: Any] = [
            "share": record.shareId,
            "volume": record.volumeId,
            "link": record.linkId,
            "label": record.label,
            "downloaded": record.downloaded,
            "total": record.total,
            "status": record.status,
            "rate": record.bytesPerSecond,
        ]
        if let error = record.error { json["error"] = error }
        let data = (try? JSONSerialization.data(withJSONObject: json, options: [.sortedKeys])) ?? Data()
        return String(decoding: data, as: UTF8.self)
    }

    /// A row that does not decode is skipped rather than failing the list.
    static func decode(_ encoded: String) -> RetainedDownload? {
        guard let object = try? JSONSerialization.jsonObject(with: Data(encoded.utf8)),
              let json = object as? [String: Any],
              let share = json["share"] as? String,
              let volume = json["volume"] as? String,
              let link = json["link"] as? String
        else { return nil }
        let error = (json["error"] as? String).flatMap { $0.trimmingCharacters(in: .whitespaces).isEmpty ? nil : $0 }
        return RetainedDownload(
            shareId: share,
            volumeId: volume,
            linkId: link,
            label: json["label"] as? String ?? link,
            downloaded: (json["downloaded"] as? NSNumber)?.int64Value ?? 0,
            total: (json["total"] as? NSNumber)?.int64Value ?? 0,
            status: json["status"] as? String ?? RetainedDownload.statusQueued,
            error: error,
            bytesPerSecond: (json["rate"] as? NSNumber)?.int64Value ?? 0
        )
    }

    /// In a directory of its own, excluded from backup like the catalog: an
    /// atomic write replaces the file, so the flag has to sit on its parent.
    private static var defaultFile: URL {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        var directory = support.appendingPathComponent("state", isDirectory: true)
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        var excluded = URLResourceValues()
        excluded.isExcludedFromBackup = true
        try? directory.setResourceValues(excluded)
        return directory.appendingPathComponent("offline-downloads.json")
    }
}

/// Bytes per second over roughly the last few seconds.
///
/// Progress arrives once per 4 MiB block, so on a slow link the bar moves in
/// steps seconds apart; a rate beside it is what tells a slow download from a
/// stuck one. Smoothed, because one block that took a little longer than the
/// last would otherwise halve the figure for a moment.
struct TransferRate {
    private var lastBytes: Int64 = -1
    private var lastAt: Int64 = 0
    private var smoothed = 0.0

    /// Record `bytes` transferred at `now` (milliseconds) and return the rate.
    mutating func sample(_ bytes: Int64, now: Int64) -> Int64 {
        if lastBytes < 0 || bytes < lastBytes {
            lastBytes = bytes
            lastAt = now
            return Int64(smoothed)
        }
        let elapsed = now - lastAt
        if elapsed <= 0 { return Int64(smoothed) }
        let instant = Double(bytes - lastBytes) * 1000 / Double(elapsed)
        smoothed = smoothed > 0 ? 0.7 * smoothed + 0.3 * instant : instant
        lastBytes = bytes
        lastAt = now
        return Int64(smoothed)
    }
}
