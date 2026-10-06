@testable import ProtonStream
import XCTest

final class DownloadStateStoreTests: XCTestCase {
    private var file: URL!
    private var store: DownloadStateStore!

    override func setUp() {
        file = FileManager.default.temporaryDirectory.appendingPathComponent("downloads-\(UUID().uuidString).json")
        store = DownloadStateStore(file: file)
    }

    override func tearDown() {
        try? FileManager.default.removeItem(at: file)
    }

    private func record(
        shareId: String = "share-1",
        linkId: String = "link-1",
        label: String = "S01E01.mkv",
        status: String = RetainedDownload.statusRunning,
        error: String? = nil
    ) -> RetainedDownload {
        RetainedDownload(
            shareId: shareId,
            volumeId: "volume-1",
            linkId: linkId,
            label: label,
            downloaded: 4 * 1024 * 1024,
            total: 761 * 1024 * 1024,
            status: status,
            error: error
        )
    }

    private func baseJson() -> [String: Any] {
        [
            "share": "share-1",
            "volume": "volume-1",
            "link": "link-1",
            "label": "S01E01.mkv",
            "downloaded": 0,
            "total": 0,
            "status": RetainedDownload.statusQueued,
        ]
    }

    /// A row as an older or another writer left it, read by a fresh store.
    private func writeRaw(_ rows: [String: String]) throws {
        try JSONEncoder().encode(rows).write(to: file)
        store = DownloadStateStore(file: file)
    }

    private func encoded(_ json: [String: Any]) throws -> String {
        String(decoding: try JSONSerialization.data(withJSONObject: json), as: UTF8.self)
    }

    func testARecordSurvivesARoundTripUnchanged() {
        let original = record()
        store.put(original)
        XCTAssertEqual(store.get("share-1", "link-1"), original)
    }

    func testARecordSurvivesARestartUnchanged() {
        let original = record()
        store.put(original)
        XCTAssertEqual(DownloadStateStore(file: file).get("share-1", "link-1"), original)
    }

    func testAnAbsentRecordReadsAsNilRatherThanADefault() {
        XCTAssertNil(store.get("share-1", "link-1"))
    }

    func testAnErrorMessageRoundTripsAndANilOneStaysNil() {
        store.put(record(error: "no space left on device"))
        XCTAssertEqual(store.get("share-1", "link-1")?.error, "no space left on device")
        store.put(record(error: nil))
        XCTAssertNil(store.get("share-1", "link-1")?.error)
    }

    func testABlankErrorIsNotReadAsAFailureMessage() throws {
        var json = baseJson()
        json["error"] = ""
        try writeRaw([downloadKey("share-1", "link-1"): encoded(json)])
        XCTAssertNil(store.get("share-1", "link-1")?.error)
    }

    func testARecordWithNoLabelIsCaptionedWithItsLinkId() throws {
        var json = baseJson()
        json["label"] = nil
        try writeRaw([downloadKey("share-1", "link-1"): encoded(json)])
        XCTAssertEqual(store.get("share-1", "link-1")?.label, "link-1")
    }

    func testACorruptRowIsSkippedRatherThanFailingTheWholeList() throws {
        try writeRaw([
            downloadKey("share-1", "link-1"): DownloadStateStore.encode(record(label: "good.mkv")),
            downloadKey("share-1", "link-broken"): "{ not json",
        ])
        XCTAssertEqual(store.records().map(\.label), ["good.mkv"])
    }

    func testRecordsAreOrderedByLabelWithoutRegardToCase() {
        store.put(record(linkId: "a", label: "beta.mkv"))
        store.put(record(linkId: "b", label: "Alpha.mkv"))
        XCTAssertEqual(store.records().map(\.label), ["Alpha.mkv", "beta.mkv"])
    }

    func testRemovingOneShareLeavesTheOthersAlone() {
        store.put(record(shareId: "share-1", linkId: "a"))
        store.put(record(shareId: "share-2", linkId: "b"))
        store.removeShare("share-1")
        XCTAssertEqual(store.records().map(\.shareId), ["share-2"])
    }

    func testClearForgetsEverything() {
        store.put(record(linkId: "a"))
        store.put(record(linkId: "b"))
        store.clear()
        XCTAssertTrue(store.records().isEmpty)
    }

    func testTheKeySeparatesShareFromLinkSoIdsCannotRunTogether() {
        store.put(record(shareId: "a", linkId: "b-c"))
        store.put(record(shareId: "a-b", linkId: "c"))
        XCTAssertEqual(store.records().count, 2)
    }

    func testAnUpdateThatReturnsNilLeavesTheRecordAlone() {
        store.put(record(status: RetainedDownload.statusPaused))
        store.update("share-1", "link-1") { _ in nil }
        XCTAssertEqual(store.get("share-1", "link-1")?.status, RetainedDownload.statusPaused)
    }
}

final class TransferRateTests: XCTestCase {
    func testTheFirstSampleHasNoRateToReport() {
        var rate = TransferRate()
        XCTAssertEqual(rate.sample(4 * 1024 * 1024, now: 1000), 0)
    }

    func testASteadyTransferReportsItsRate() {
        var rate = TransferRate()
        _ = rate.sample(0, now: 0)
        XCTAssertEqual(rate.sample(1_000_000, now: 1000), 1_000_000)
        XCTAssertEqual(rate.sample(2_000_000, now: 2000), 1_000_000)
    }

    func testOneSlowBlockMovesTheRateWithoutHalvingIt() {
        var rate = TransferRate()
        _ = rate.sample(0, now: 0)
        _ = rate.sample(1_000_000, now: 1000)
        let after = rate.sample(2_000_000, now: 3000)
        XCTAssertTrue((600_000 ... 999_999).contains(after))
    }

    func testAResumedDownloadStartingBelowTheLastSampleStartsOver() {
        var rate = TransferRate()
        _ = rate.sample(0, now: 0)
        _ = rate.sample(1_000_000, now: 1000)
        XCTAssertEqual(rate.sample(0, now: 2000), 1_000_000)
    }
}

final class DownloadStatusLineTests: XCTestCase {
    func testARunningDownloadSaysHowFarAndHowFast() {
        let download = RetainedDownload(
            shareId: "s", volumeId: "v", linkId: "l", label: "E01",
            downloaded: 120 * 1024 * 1024, total: 1288 * 1024 * 1024,
            status: RetainedDownload.statusRunning, bytesPerSecond: 2_516_582
        )
        XCTAssertEqual(downloadStatusLine(download), "Running · 120.0 MiB of 1.3 GiB · 2.4 MiB/s")
    }

    func testAQueuedDownloadSaysOnlyThat() {
        let download = RetainedDownload(shareId: "s", volumeId: "v", linkId: "l", label: "E01")
        XCTAssertEqual(downloadStatusLine(download), "Queued")
    }

    func testAPausedDownloadGivesNoRate() {
        let download = RetainedDownload(
            shareId: "s", volumeId: "v", linkId: "l", label: "E01",
            downloaded: 1024, total: 2048, status: RetainedDownload.statusPaused, bytesPerSecond: 500
        )
        XCTAssertEqual(downloadStatusLine(download), "Paused · 1.0 KiB of 2.0 KiB")
    }
}
