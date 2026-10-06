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

/// Why the open file ended, mirroring `pstr_player::EndReason` so every client
/// agrees on what "finished" means.
///
/// Only `eof` is an episode watched to the end. Treating the others as one — a
/// stop, a shutdown, a failed demux — is what marks an episode that never
/// played as watched and autoplays past it.
enum EndReason {
    case eof, stopped, quit, failed, other
}

struct MpvTrack: Identifiable, Hashable {
    let id: Int64
    let type: String
    let language: String
    let title: String
    let selected: Bool

    var label: String {
        let parts = [language, title].filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
        return parts.isEmpty ? "\(type.prefix(1).uppercased() + type.dropFirst()) \(id)" : parts.joined(separator: " — ")
    }
}

/// One libmpv core rendering into a Metal layer, for the life of the app.
///
/// Every public method runs on the main thread; mpv's events are read on a
/// thread of their own and applied back on the main thread. The state mirrors
/// what `pstr_mpv.cpp` publishes on Android, field for field.
@MainActor
@Observable
final class MpvPlayer {
    private(set) var position: Double = 0
    private(set) var duration: Double = 0
    private(set) var volume: Double = 100
    private(set) var paused = false
    private(set) var muted = false
    /// The file ended, and why. Nil while one is open.
    private(set) var endReason: EndReason?
    /// mpv has run out of buffered data and stopped to refill — not the same
    /// as `paused`, which is the viewer's doing.
    private(set) var buffering = false
    /// How full the demuxer cache is while `buffering`, 0–100.
    private(set) var cachePercent: Double = 0
    /// A seek has been issued but playback has not resumed at the new position.
    private(set) var seeking = false
    /// How far the demuxer has read ahead, in seconds into the file.
    private(set) var cachedUntil: Double = 0
    /// The picture's display size, or zero until mpv has decoded enough.
    private(set) var videoWidth: Double = 0
    private(set) var videoHeight: Double = 0
    private(set) var tracks: [MpvTrack] = []
    private(set) var chapters: [ChapterRecord] = []
    /// Which episode the readings above describe, or nil between `loadfile`
    /// and mpv's `START_FILE`. Everything that *writes* per-episode state —
    /// the resume position above all — must check it: one core serves every
    /// episode, and a reading attributed to the wrong one resumes the next
    /// episode where the last one stopped.
    private(set) var media: String?
    /// What mpv last complained about, until the viewer dismisses it.
    var problem: String?
    /// Bumped by every load, so a reader can tell "episode two, second zero"
    /// from a leftover reading of episode one.
    private(set) var generation = 0

    /// Whether the picture is stopped for a reason the viewer did not choose.
    var stalled: Bool { (buffering || seeking) && !paused && endReason == nil }

    /// Called once per file that ends, never for the one a load replaced.
    @ObservationIgnored var onEnded: ((EndReason, String) -> Void)?
    /// Called on every position change, for what has to react to the clock.
    @ObservationIgnored var onTick: (() -> Void)?
    /// Called when the duration or the pause state changes: what the lock
    /// screen shows, and whether the screen may sleep.
    @ObservationIgnored var onStateChanged: (() -> Void)?

    @ObservationIgnored let layer = MpvMetalLayer()
    @ObservationIgnored private var mpv: OpaquePointer?
    /// Between `loadfile` and `START_FILE`. Everything mpv reports in that
    /// window still describes the *outgoing* file, including the
    /// `END_FILE(stop)` that stopping it emits.
    @ObservationIgnored private var loading = false
    @ObservationIgnored private var pendingMedia: String?

    /// A core, or nil when libmpv would not start.
    static func create() -> MpvPlayer? {
        let player = MpvPlayer()
        return player.mpv == nil ? nil : player
    }

    private init() {
        layer.framebufferOnly = true
        layer.backgroundColor = UIColor.black.cgColor
        guard let mpv = mpv_create() else { return }
        // mpv takes the layer's address as an int64, not a Swift reference.
        var wid = Int64(Int(bitPattern: Unmanaged.passUnretained(layer).toOpaque()))
        mpv_set_option(mpv, "wid", MPV_FORMAT_INT64, &wid)
        for (name, value) in [
            ("config", "no"),
            ("vo", "gpu-next"),
            ("gpu-api", "vulkan"),
            ("gpu-context", "moltenvk"),
            // The default; `load` sets it per file from the viewer's setting.
            ("hwdec", "videotoolbox"),
            ("video-rotate", "no"),
            ("keep-open", "no"),
            ("cache", "yes"),
            ("cache-secs", "30"),
            ("demuxer-readahead-secs", "30"),
            ("demuxer-max-bytes", "50331648"),
            ("subs-fallback", "yes"),
            // AVAudioSession is this app's to manage, as audio focus is
            // Android's.
            ("audio-exclusive", "no"),
        ] {
            mpv_set_option_string(mpv, name, value)
        }
        guard registerStreamProtocol(mpv), mpv_initialize(mpv) >= 0 else {
            mpv_terminate_destroy(mpv)
            return
        }
        for (id, name, format) in Self.observed {
            mpv_observe_property(mpv, id, name, format)
        }
        // Errors mpv reports about the file itself, rather than about the
        // request that opened it. Without these a failed demux is silent.
        mpv_request_log_messages(mpv, "error")
        self.mpv = mpv

        let thread = Thread { [weak self] in Self.eventLoop(mpv) { event in self?.apply(event) } }
        thread.name = "mpv-events"
        thread.start()
    }

    private static let observed: [(UInt64, String, mpv_format)] = [
        (1, "time-pos", MPV_FORMAT_DOUBLE),
        (2, "duration", MPV_FORMAT_DOUBLE),
        (3, "pause", MPV_FORMAT_FLAG),
        (4, "volume", MPV_FORMAT_DOUBLE),
        (5, "mute", MPV_FORMAT_FLAG),
        (6, "video-params/dw", MPV_FORMAT_INT64),
        (7, "video-params/dh", MPV_FORMAT_INT64),
        (8, "paused-for-cache", MPV_FORMAT_FLAG),
        (9, "cache-buffering-state", MPV_FORMAT_INT64),
        (10, "demuxer-cache-time", MPV_FORMAT_DOUBLE),
        (11, "track-list/count", MPV_FORMAT_INT64),
        (12, "chapter-list/count", MPV_FORMAT_INT64),
        (13, "seeking", MPV_FORMAT_FLAG),
    ]

    // MARK: Commands

    /// Load `pstr://<handle>` as `media`. On `false` nothing was issued, and
    /// the caller still owns the handle's release.
    func load(
        media: String,
        handle: UInt64,
        start: Double,
        audio: String?,
        subtitle: String?,
        subtitles: Bool,
        hardwareDecoding: Bool
    ) -> Bool {
        guard mpv != nil else { return false }
        // Per file, before `loadfile`: a file VideoToolbox decodes wrongly is
        // fixed from the next episode without restarting the core.
        setString("hwdec", hardwareDecoding ? "videotoolbox" : "no")
        setString("start", start > 0 ? String(format: "%.3f", start) : "none")
        setString("alang", audio ?? "")
        setString("slang", subtitle ?? "")
        setString("sid", subtitles ? "auto" : "no")
        // Retire the outgoing file's readings *before* the new one loads: mpv
        // only republishes them once it has demuxed enough of the stream, and
        // until then every reader would see the last episode's clock under the
        // new episode's name. Volume, mute and pause are the core's, not the
        // file's, and carry across.
        position = start
        duration = 0
        endReason = nil
        buffering = false
        cachePercent = 0
        seeking = false
        cachedUntil = 0
        videoWidth = 0
        videoHeight = 0
        tracks = []
        chapters = []
        problem = nil
        self.media = nil
        generation += 1
        pendingMedia = media
        loading = true
        if command(["loadfile", "pstr://\(handle)", "replace"]) < 0 {
            // Nothing was issued, so no START_FILE will arrive to clear this.
            loading = false
            return false
        }
        return true
    }

    /// Written through rather than left to the observer: a tap that only shows
    /// its effect on mpv's next property event reads as a button that missed.
    func setPaused(_ value: Bool) {
        setFlag("pause", value)
        paused = value
    }

    func seek(to seconds: Double) {
        setDouble("time-pos", max(0, seconds))
        seeking = true
    }

    func setSpeed(_ value: Double) { setDouble("speed", min(max(value, 0.25), 4)) }
    func setVolume(_ value: Double) { setDouble("volume", min(max(value, 0), 100)) }
    func setMuted(_ value: Bool) { setFlag("mute", value) }

    /// A track, or none: `nil` turns subtitles off.
    func select(type: String, track: MpvTrack?) {
        setString(type == "audio" ? "aid" : "sid", track.map { String($0.id) } ?? "no")
        refreshTracks()
    }

    /// Showing no picture while backgrounded keeps the audio playing: iOS
    /// refuses GPU work from a background app, and would end the process for it.
    func setVideoEnabled(_ enabled: Bool) { setString("vid", enabled ? "auto" : "no") }

    /// Stop the open file. The stream behind it is released by mpv closing it,
    /// which this causes; releasing it from here would race that close.
    func stop() {
        _ = command(["stop"])
        media = nil
        pendingMedia = nil
        chapters = []
        tracks = []
    }

    // MARK: Events

    private enum Event {
        case double(UInt64, Double)
        case flag(UInt64, Bool)
        case int(UInt64, Int64)
        case unavailable(UInt64)
        case started
        case loaded
        case ended(Int64)
        case restarted
        case log(prefix: String, text: String)
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
                guard let property = event.data?.assumingMemoryBound(to: mpv_event_property.self).pointee else { break }
                guard let data = property.data else {
                    decoded = .unavailable(event.reply_userdata)
                    break
                }
                switch property.format {
                case MPV_FORMAT_DOUBLE: decoded = .double(event.reply_userdata, data.load(as: Double.self))
                case MPV_FORMAT_FLAG: decoded = .flag(event.reply_userdata, data.load(as: Int32.self) != 0)
                case MPV_FORMAT_INT64: decoded = .int(event.reply_userdata, data.load(as: Int64.self))
                default: decoded = .unavailable(event.reply_userdata)
                }
            case MPV_EVENT_START_FILE:
                decoded = .started
            case MPV_EVENT_FILE_LOADED:
                decoded = .loaded
            case MPV_EVENT_PLAYBACK_RESTART:
                decoded = .restarted
            case MPV_EVENT_END_FILE:
                if let end = event.data?.assumingMemoryBound(to: mpv_event_end_file.self).pointee {
                    decoded = .ended(Int64(end.reason.rawValue))
                }
            case MPV_EVENT_LOG_MESSAGE:
                if let message = event.data?.assumingMemoryBound(to: mpv_event_log_message.self).pointee,
                   let text = message.text {
                    decoded = .log(
                        prefix: message.prefix.map { String(cString: $0) } ?? "",
                        text: String(cString: text).trimmingCharacters(in: .whitespacesAndNewlines)
                    )
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
            switch id {
            case 1:
                position = value
                onTick?()
            case 2:
                duration = value
                onStateChanged?()
            case 4: volume = value
            case 10: cachedUntil = value
            default: break
            }
        case let .flag(id, value):
            switch id {
            case 3:
                paused = value
                onStateChanged?()
            case 5: muted = value
            case 8: if !loading { buffering = value }
            case 13: if !loading { seeking = value }
            default: break
            }
        case let .int(id, value):
            if loading { return }
            switch id {
            case 6: videoWidth = Double(value)
            case 7: videoHeight = Double(value)
            case 9: cachePercent = Double(value)
            case 11: refreshTracks()
            case 12: refreshChapters()
            default: break
            }
        case let .unavailable(id):
            if loading { return }
            if id == 6 || id == 7 { videoWidth = 0; videoHeight = 0 }
        case .started:
            // From here on, what mpv reports is the file just loaded.
            if loading {
                loading = false
                media = pendingMedia
            }
        case .loaded:
            refreshTracks()
            refreshChapters()
        case .restarted:
            seeking = false
        case let .ended(reason):
            // The outgoing file's END_FILE arrives after the next load began.
            if loading { return }
            let ended: EndReason = switch reason {
            case Int64(MPV_END_FILE_REASON_EOF.rawValue): .eof
            case Int64(MPV_END_FILE_REASON_STOP.rawValue): .stopped
            case Int64(MPV_END_FILE_REASON_QUIT.rawValue): .quit
            case Int64(MPV_END_FILE_REASON_ERROR.rawValue): .failed
            default: .other
            }
            endReason = ended
            if ended == .failed { problem = problem ?? "This episode could not be played." }
            if let media { onEnded?(ended, media) }
        case let .log(prefix, text):
            // The outgoing file's complaints are not the new one's.
            if loading { return }
            // The first complaint per file, and not ffmpeg's: its decoders log
            // recoverable glitches at error level all through a healthy file.
            if text.isEmpty || prefix.hasPrefix("ffmpeg") || problem != nil { return }
            problem = text
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
