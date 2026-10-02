import Foundation
import Libmpv
import Observation
import UIKit

/// MoltenVK can set the drawable to 1x1 to force a presentation through, which
/// flickers and can stick. MPVKit's demo carries the same guard
/// (mpv-player/mpv#13651).
final class MpvMetalLayer: CAMetalLayer {
    override var drawableSize: CGSize {
        get { super.drawableSize }
        set {
            if Int(newValue.width) > 1, Int(newValue.height) > 1 {
                super.drawableSize = newValue
            }
        }
    }
}

struct MpvTrack: Identifiable, Hashable {
    let id: Int64
    let type: String
    let language: String
    let title: String
    let selected: Bool

    var label: String {
        let parts = [title, language.uppercased()].filter { !$0.isEmpty }
        return parts.isEmpty ? "Track \(id)" : parts.joined(separator: " · ")
    }
}

/// One libmpv core rendering into a Metal layer. Every public method runs on
/// the main thread; mpv's events are read on a thread of their own and applied
/// back on the main thread.
@MainActor
@Observable
final class MpvPlayer {
    enum EndReason { case eof, stopped, failed }

    private(set) var position: Double = 0
    private(set) var duration: Double = 0
    private(set) var paused = false
    private(set) var buffering = false
    private(set) var tracks: [MpvTrack] = []
    private(set) var chapters: [ChapterRecord] = []
    private(set) var failure: String?
    /// Between `loadfile` and mpv's `FILE_LOADED`. Everything mpv reports in
    /// that window still describes the *outgoing* file, including the
    /// `END_FILE(stop)` stopping it emits.
    private(set) var loading = false

    /// Called once per file that ends, never for the one a load replaced.
    @ObservationIgnored var onEnded: ((EndReason) -> Void)?

    @ObservationIgnored let layer = MpvMetalLayer()
    @ObservationIgnored private var mpv: OpaquePointer?

    init() {
        layer.framebufferOnly = true
        layer.backgroundColor = UIColor.black.cgColor
        guard let mpv = mpv_create() else {
            failure = "libmpv did not start"
            return
        }
        var wid = layer
        mpv_set_option(mpv, "wid", MPV_FORMAT_INT64, &wid)
        for (name, value) in [
            ("config", "no"),
            ("vo", "gpu-next"),
            ("gpu-api", "vulkan"),
            ("gpu-context", "moltenvk"),
            ("hwdec", "videotoolbox"),
            ("video-rotate", "no"),
            ("keep-open", "no"),
            ("cache", "yes"),
            ("cache-secs", "30"),
            ("demuxer-readahead-secs", "30"),
            ("demuxer-max-bytes", "50331648"),
            ("subs-fallback", "yes"),
        ] {
            mpv_set_option_string(mpv, name, value)
        }
        guard registerStreamProtocol(mpv), mpv_initialize(mpv) >= 0 else {
            mpv_terminate_destroy(mpv)
            failure = "libmpv did not initialize"
            return
        }
        mpv_observe_property(mpv, 1, "time-pos", MPV_FORMAT_DOUBLE)
        mpv_observe_property(mpv, 2, "duration", MPV_FORMAT_DOUBLE)
        mpv_observe_property(mpv, 3, "pause", MPV_FORMAT_FLAG)
        mpv_observe_property(mpv, 4, "paused-for-cache", MPV_FORMAT_FLAG)
        mpv_observe_property(mpv, 5, "track-list/count", MPV_FORMAT_INT64)
        mpv_observe_property(mpv, 6, "chapter-list/count", MPV_FORMAT_INT64)
        mpv_request_log_messages(mpv, "error")
        self.mpv = mpv

        let thread = Thread { [weak self] in Self.eventLoop(mpv) { event in self?.apply(event) } }
        thread.name = "mpv-events"
        thread.start()
    }

    // MARK: Commands

    /// Load `pstr://<handle>`. On `false` nothing was issued, and the caller
    /// still owns the handle's release.
    func load(handle: UInt64, start: Double, audio: String?, subtitle: String?, subtitles: Bool) -> Bool {
        guard mpv != nil else { return false }
        setString("start", start > 0 ? String(format: "%.3f", start) : "none")
        setString("alang", audio ?? "")
        setString("slang", subtitle ?? "")
        setString("sid", subtitles ? "auto" : "no")
        position = 0
        duration = 0
        tracks = []
        chapters = []
        failure = nil
        loading = true
        if command(["loadfile", "pstr://\(handle)", "replace"]) < 0 {
            loading = false
            return false
        }
        return true
    }

    func setPaused(_ value: Bool) { setFlag("pause", value) }
    func seek(to seconds: Double) { setDouble("time-pos", max(0, seconds)) }
    func seek(by seconds: Double) { _ = command(["seek", String(seconds), "relative"]) }
    func setSpeed(_ value: Double) { setDouble("speed", min(max(value, 0.25), 4)) }
    func setVolume(_ value: Double) { setDouble("volume", min(max(value, 0), 100)) }
    func setMuted(_ value: Bool) { setFlag("mute", value) }

    func select(audio: Bool, track: MpvTrack?) {
        let property = audio ? "aid" : "sid"
        if let track { setString(property, String(track.id)) } else { setString(property, "no") }
        refreshTracks()
    }

    /// Showing no picture while backgrounded keeps the audio playing and avoids
    /// the black frame MoltenVK otherwise leaves on return.
    func setVideoEnabled(_ enabled: Bool) { setString("vid", enabled ? "auto" : "no") }

    /// Stop and tear the core down. `cancel_fn` interrupts a read parked in a
    /// block fetch, and the event thread destroys the core once it is down.
    func shutdown() {
        guard let mpv else { return }
        self.mpv = nil
        mpv_command_string(mpv, "quit")
    }

    // MARK: Events

    private enum Event {
        case double(UInt64, Double)
        case flag(UInt64, Bool)
        case count(UInt64)
        case loaded
        case ended(Int32)
        case log(String)
    }

    /// Runs until the core shuts down, then destroys it. Nothing else calls
    /// `mpv_terminate_destroy`, so no main-thread call can race it.
    private nonisolated static func eventLoop(_ mpv: OpaquePointer, _ deliver: @escaping @MainActor (Event) -> Void) {
        while true {
            guard let event = mpv_wait_event(mpv, -1)?.pointee else { continue }
            var decoded: Event?
            switch event.event_id {
            case MPV_EVENT_SHUTDOWN:
                mpv_terminate_destroy(mpv)
                return
            case MPV_EVENT_PROPERTY_CHANGE:
                guard let property = event.data?.assumingMemoryBound(to: mpv_event_property.self).pointee,
                      let data = property.data else { break }
                switch property.format {
                case MPV_FORMAT_DOUBLE: decoded = .double(event.reply_userdata, data.load(as: Double.self))
                case MPV_FORMAT_FLAG: decoded = .flag(event.reply_userdata, data.load(as: Int32.self) != 0)
                case MPV_FORMAT_INT64: decoded = .count(event.reply_userdata)
                default: break
                }
            case MPV_EVENT_FILE_LOADED:
                decoded = .loaded
            case MPV_EVENT_END_FILE:
                if let end = event.data?.assumingMemoryBound(to: mpv_event_end_file.self).pointee {
                    decoded = .ended(end.reason.rawValue)
                }
            case MPV_EVENT_LOG_MESSAGE:
                if let message = event.data?.assumingMemoryBound(to: mpv_event_log_message.self).pointee,
                   let text = message.text {
                    decoded = .log(String(cString: text).trimmingCharacters(in: .whitespacesAndNewlines))
                }
            default:
                break
            }
            if let decoded {
                DispatchQueue.main.async { deliver(decoded) }
            }
        }
    }

    private func apply(_ event: Event) {
        switch event {
        case let .double(id, value):
            if loading { return }
            if id == 1 { position = value } else if id == 2 { duration = value }
        case let .flag(id, value):
            if id == 3 { paused = value } else if id == 4 { buffering = value }
        case let .count(id):
            if loading { return }
            if id == 5 { refreshTracks() } else if id == 6 { refreshChapters() }
        case .loaded:
            loading = false
            refreshTracks()
            refreshChapters()
        case let .ended(reason):
            // The outgoing file's END_FILE arrives after the next load began.
            if loading { return }
            switch reason {
            case MPV_END_FILE_REASON_EOF.rawValue: onEnded?(.eof)
            case MPV_END_FILE_REASON_ERROR.rawValue:
                failure = failure ?? "The file could not be played"
                onEnded?(.failed)
            default: onEnded?(.stopped)
            }
        case let .log(text):
            if !text.isEmpty { failure = text }
        }
    }

    private func refreshTracks() {
        let count = getInt("track-list/count")
        tracks = (0 ..< max(0, count)).map { index in
            let base = "track-list/\(index)/"
            return MpvTrack(
                id: getInt(base + "id"),
                type: getString(base + "type") ?? "",
                language: getString(base + "lang") ?? "",
                title: getString(base + "title") ?? "",
                selected: getString(base + "selected") == "yes"
            )
        }
    }

    private func refreshChapters() {
        let count = getInt("chapter-list/count")
        chapters = (0 ..< max(0, count)).map { index in
            let base = "chapter-list/\(index)/"
            return ChapterRecord(index: index, title: getString(base + "title"), start: getDouble(base + "time"))
        }
    }

    // MARK: Property plumbing

    private func command(_ args: [String]) -> Int32 {
        guard let mpv else { return -1 }
        var pointers = args.map { UnsafePointer<CChar>(strdup($0)) }
        pointers.append(nil)
        defer { pointers.forEach { free(UnsafeMutablePointer(mutating: $0)) } }
        return mpv_command(mpv, &pointers)
    }

    private func setString(_ name: String, _ value: String) {
        guard let mpv else { return }
        mpv_set_property_string(mpv, name, value)
    }

    private func setDouble(_ name: String, _ value: Double) {
        guard let mpv else { return }
        var value = value
        mpv_set_property(mpv, name, MPV_FORMAT_DOUBLE, &value)
    }

    private func setFlag(_ name: String, _ value: Bool) {
        guard let mpv else { return }
        var flag: Int32 = value ? 1 : 0
        mpv_set_property(mpv, name, MPV_FORMAT_FLAG, &flag)
    }

    private func getInt(_ name: String) -> Int64 {
        guard let mpv else { return 0 }
        var value: Int64 = 0
        mpv_get_property(mpv, name, MPV_FORMAT_INT64, &value)
        return value
    }

    private func getDouble(_ name: String) -> Double {
        guard let mpv else { return 0 }
        var value = 0.0
        mpv_get_property(mpv, name, MPV_FORMAT_DOUBLE, &value)
        return value
    }

    private func getString(_ name: String) -> String? {
        guard let mpv, let raw = mpv_get_property_string(mpv, name) else { return nil }
        defer { mpv_free(raw) }
        return String(cString: raw)
    }
}
