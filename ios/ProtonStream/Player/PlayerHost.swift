import AVFoundation
import Foundation
import MediaPlayer
import Observation
import UIKit

/// How long the "up next" card counts down before it loads the next episode.
let upNextSeconds = 10

/// Past this much of an episode it counts as watched.
private let watchedFraction = 0.9

/// How an episode is named in watch state and player readings.
func mediaKey(_ episode: EpisodeRecord) -> String { "\(episode.shareId):\(episode.linkId)" }

/// The open episode's stream, from Rust: opened, handed to mpv, given back.
///
/// The native handle belongs to mpv once a load succeeds — `close_fn` in the
/// stream protocol releases it after mpv's last read. Only a load that never
/// happened leaves it here to release.
private final class StreamSession {
    let episode: EpisodeRecord
    private(set) var handle: UInt64?
    private(set) var size: UInt64 = 0

    init(episode: EpisodeRecord) {
        self.episode = episode
    }

    func open() async throws {
        let stream = try await NativeRuntime.engine().openStream(shareId: episode.shareId, volumeId: episode.volumeId, linkId: episode.linkId)
        size = stream.size()
        handle = stream.nativeHandle()
    }

    /// The player now owns the published handle until stop, end or close.
    func transferToPlayer() {
        handle = nil
    }

    /// Give the episode's stream back to Rust, from a task of its own: the one
    /// moment this has to run is when the player moves on, and nothing that
    /// goes away with it may cancel it.
    func close() {
        if let handle { pstr_android_stream_release(handle) }
        handle = nil
        let episode = episode
        Task.detached {
            try? await NativeRuntime.engine().releaseStream(shareId: episode.shareId, volumeId: episode.volumeId, linkId: episode.linkId)
        }
    }
}

/// What the player has open and everything that walks it: the stream per
/// episode, the resume position, autoplay, auto-skip, the lock screen and the
/// audio session.
///
/// Android splits this between `PlayerScreen`, `NativeMpvHost` and
/// `PlaybackService`. Here it is one object that outlives the player's view,
/// because the episode keeps playing while the view is minimised or the app
/// is in the background.
@MainActor
@Observable
final class PlayerHost {
    static let shared = PlayerHost()

    /// The core, once something has been played. Nil if libmpv would not start.
    private(set) var player: MpvPlayer?
    private(set) var unavailable = false
    private(set) var title: TitleRecord?
    private(set) var episodes: [EpisodeRecord] = []
    private(set) var index = 0
    /// Playing, but not on screen: the episode keeps going and the mini
    /// transport is what says so.
    var minimized = false
    /// The open file's chapters, already resolved into openings and endings.
    private(set) var chapters = ChapterPlan(entries: [], creditsStart: nil)
    /// What stopped this episode, until the viewer dismisses it.
    var playbackError: String?
    /// Read once per episode: a preference the viewer changes mid-episode
    /// applies to the next one.
    private(set) var autoSkip = false
    private(set) var autoplay = true
    /// The rate playing, so the speed list opens on it.
    var speed = 1.0

    /// Persist where an episode was left: position, duration, watched.
    @ObservationIgnored var onSaveProgress: ((EpisodeRecord, Double, Double, Bool) -> Void)?
    /// The player closed: where it stopped is what another device wants next.
    @ObservationIgnored var onClosed: (() -> Void)?

    @ObservationIgnored private var session: StreamSession?
    @ObservationIgnored private var loadTask: Task<Void, Never>?
    @ObservationIgnored private var saveTask: Task<Void, Never>?
    @ObservationIgnored private var plannedFor: (Double, Int)?
    /// Chapters already auto-skipped in this episode: seeking back into an
    /// opening on purpose must not be undone by the setting that skipped it.
    @ObservationIgnored private var skipped = Set<Int64>()
    @ObservationIgnored private var pausedByInterruption = false
    @ObservationIgnored private var artworkFor: String?

    private init() {
        observeAudioSession()
        configureRemoteCommands()
    }

    var episode: EpisodeRecord? { episodes.indices.contains(index) ? episodes[index] : nil }
    var key: String? { episode.map(mediaKey) }
    var next: EpisodeRecord? { episodes.indices.contains(index + 1) ? episodes[index + 1] : nil }
    var isOpen: Bool { title != nil }

    // MARK: - Opening

    /// Open a title's episodes at `index`, or move to it if the title is open.
    func play(_ title: TitleRecord, at index: Int) {
        let episodes = title.playlist
        guard !episodes.isEmpty else { return }
        let target = min(max(index, 0), episodes.count - 1)
        // The same episode, still open: only bring it back on screen. One that
        // has ended has nothing loaded, and is played again.
        if self.title?.key == title.key, mediaKey(episodes[target]) == key, player?.endReason == nil {
            self.title = title
            self.episodes = episodes
            minimized = false
            return
        }
        save()
        self.title = title
        self.episodes = episodes
        self.index = target
        minimized = false
        load()
    }

    /// Keep the open title's records current after the library reloads, so the
    /// episode list shows what was just watched.
    func update(titles: [TitleRecord]) {
        guard let key = title?.key, let fresh = titles.first(where: { $0.key == key }) else { return }
        let current = self.key
        title = fresh
        episodes = fresh.playlist
        if let current, let moved = episodes.firstIndex(where: { mediaKey($0) == current }) { index = moved }
    }

    func advance() {
        guard next != nil else { return }
        move(to: index + 1)
    }

    func retreat() {
        guard index > 0 else { return }
        move(to: index - 1)
    }

    func jump(to target: Int) {
        guard target != index, episodes.indices.contains(target) else { return }
        move(to: target)
    }

    private func move(to target: Int) {
        save()
        index = target
        load()
    }

    /// Stop playback and close the player. The mini transport's ✕ and the
    /// player's own are the one place it is unambiguous.
    func close() {
        save()
        loadTask?.cancel()
        saveTask?.cancel()
        player?.stop()
        session?.close()
        session = nil
        title = nil
        episodes = []
        index = 0
        minimized = false
        playbackError = nil
        chapters = ChapterPlan(entries: [], creditsStart: nil)
        OrientationLock.set(false)
        MPNowPlayingInfoCenter.default().nowPlayingInfo = nil
        UIApplication.shared.isIdleTimerDisabled = false
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        onClosed?()
    }

    private func load() {
        guard let title, let episode else { return }
        loadTask?.cancel()
        session?.close()
        playbackError = nil
        chapters = ChapterPlan(entries: [], creditsStart: nil)
        plannedFor = nil
        skipped = []
        let session = StreamSession(episode: episode)
        self.session = session
        let key = mediaKey(episode)
        let titleKey = title.key
        publishNowPlaying()
        if player == nil, !unavailable {
            player = MpvPlayer.create()
            unavailable = player == nil
            player?.onEnded = { [weak self] reason, media in self?.ended(reason, media) }
            player?.onTick = { [weak self] in self?.tick() }
            player?.onStateChanged = { [weak self] in self?.publishNowPlaying() }
        }
        guard let player else {
            playbackError = "Playback is unavailable: libmpv did not start."
            return
        }
        try? AVAudioSession.sharedInstance().setActive(true)
        loadTask = Task {
            do {
                try await session.open()
                let startup = try await Task.detached {
                    let engine = try NativeRuntime.blockingEngine()
                    // A choice made on this title wins; without one, the choice
                    // the viewer made anywhere else does — the desktop client's
                    // fallback chain.
                    let show = try engine.titleTrackPreferences(titleKey: titleKey)
                    let global = try engine.playbackPrefs()
                    let watch = try engine.watchState(shareId: episode.shareId, linkId: episode.linkId)
                    return (show, global, watch)
                }.value
                if Task.isCancelled || self.session !== session {
                    abandon(session)
                    return
                }
                let (show, global, watch) = startup
                guard let handle = session.handle, session.size > 0 else { throw PlaybackFailure("Cannot play an empty stream") }
                autoSkip = global.autoSkip
                autoplay = global.autoplayNext
                speed = global.speed
                let loaded = player.load(
                    media: key,
                    handle: handle,
                    // A finished episode restarts rather than resuming three
                    // seconds from its own credits.
                    start: watch.flatMap { $0.watched ? nil : $0.positionSecs } ?? 0,
                    audio: show?.audioLanguage ?? global.audioLanguage,
                    subtitle: show?.subtitleLanguage ?? global.subtitleLanguage,
                    subtitles: show?.subtitles ?? global.subtitles,
                    hardwareDecoding: SettingsStore().hardwareDecoding
                )
                guard loaded else { throw PlaybackFailure("libmpv rejected the stream") }
                session.transferToPlayer()
                // After the load, because mpv keeps none of the three across one.
                player.setVolume(global.volume)
                player.setMuted(global.muted)
                player.setSpeed(global.speed)
                player.setPaused(false)
                startSaving()
            } catch {
                if Task.isCancelled || self.session !== session {
                    abandon(session)
                    return
                }
                playbackError = errorMessage(error)
            }
        }
    }

    /// A session the player moved past while it was still opening. Its
    /// `close()` already ran, before there was a handle to release, so the one
    /// it has published since is released here — and only that, since the
    /// stream itself was already given back.
    private func abandon(_ session: StreamSession) {
        if let handle = session.handle {
            pstr_android_stream_release(handle)
            session.transferToPlayer()
        }
    }

    private struct PlaybackFailure: LocalizedError {
        let errorDescription: String?
        init(_ message: String) { errorDescription = message }
    }

    // MARK: - Progress

    /// Resume position, written only from readings that carry this episode's
    /// name: an unattributed reading is the previous one's clock.
    func save() {
        guard let player, let episode, player.media == mediaKey(episode), player.duration > 0 else { return }
        onSaveProgress?(
            episode,
            min(max(player.position, 0), player.duration),
            player.duration,
            // Watched means played to the end. An episode that was stopped, or
            // that mpv could not play at all, also "ended" — recording either
            // as watched retires an episode nobody saw.
            player.endReason == .eof || player.position >= player.duration * watchedFraction
        )
    }

    private func startSaving() {
        saveTask?.cancel()
        saveTask = Task {
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(5))
                if Task.isCancelled { return }
                save()
            }
        }
    }

    // MARK: - Reacting to the player

    private func ended(_ reason: EndReason, _ media: String) {
        guard media == key else { return }
        save()
        // Autoplay fires on a clean end of file only — never on a failure.
        if reason == .eof, autoplay, next != nil {
            advance()
        } else if reason == .failed {
            playbackError = player?.problem ?? "This episode could not be played."
        }
        publishNowPlaying()
    }

    private func tick() {
        guard let player else { return }
        replanChapters(player)
        // Auto-skip, once per chapter.
        if autoSkip, !player.paused, let offer = skipOffer(plan: chapters, position: player.position),
           let chapter = chapters.entries.last(where: { player.position + 0.001 >= $0.start }),
           skipped.insert(chapter.index).inserted {
            player.seek(to: offer.target)
        }
    }

    /// Re-read the chapters when there is a reason to: a different file, or a
    /// duration that has only just arrived. Both change the answer — the length
    /// of a chapter is what settles whether `Intro` is an opening or ten minutes
    /// of story — and neither changes again after that.
    private func replanChapters(_ player: MpvPlayer) {
        let wanted = (player.duration, player.generation)
        if let plannedFor, plannedFor == wanted, !(chapters.entries.isEmpty && !player.chapters.isEmpty) { return }
        plannedFor = wanted
        chapters = chapterPlan(chapters: player.chapters, duration: player.duration > 0 ? player.duration : nil)
    }

    /// The skip button's offer at the current position, if any.
    var offer: SkipOffer? {
        guard let player else { return nil }
        return skipOffer(plan: chapters, position: player.position)
    }

    // MARK: - Transport

    func setPaused(_ paused: Bool) {
        player?.setPaused(paused)
        if !paused { try? AVAudioSession.sharedInstance().setActive(true) }
        publishNowPlaying()
    }

    func seek(to seconds: Double) {
        player?.seek(to: seconds)
        updateElapsed(seconds)
    }

    /// Persist a volume or a mute, leaving whichever was not given as it was.
    /// Written when the thumb is released rather than while it moves.
    func rememberVolume(volume: Double? = nil, muted: Bool? = nil) {
        Task.detached {
            guard let engine = try? NativeRuntime.blockingEngine(), var prefs = try? engine.playbackPrefs() else { return }
            prefs.volume = volume ?? prefs.volume
            prefs.muted = muted ?? prefs.muted
            try? engine.setPlaybackPrefs(prefs: prefs)
        }
    }

    /// Persist a playback rate, so the next episode starts at it.
    func setSpeed(_ rate: Double) {
        speed = rate
        player?.setSpeed(rate)
        publishNowPlaying()
        Task.detached {
            guard let engine = try? NativeRuntime.blockingEngine(), var prefs = try? engine.playbackPrefs() else { return }
            prefs.speed = rate
            try? engine.setPlaybackPrefs(prefs: prefs)
        }
    }

    /// A track for this title. The choice is the title's from now on.
    func select(type: String, track: MpvTrack?) {
        player?.select(type: type, track: track)
        guard let titleKey = title?.key else { return }
        Task.detached {
            guard let engine = try? NativeRuntime.blockingEngine(), let global = try? engine.playbackPrefs() else { return }
            // No per-title choice yet: start from what plays everywhere else,
            // so choosing an audio track does not silently drop the subtitle
            // preference this episode started with.
            var preferences = (try? engine.titleTrackPreferences(titleKey: titleKey)) ?? TrackPreferencesRecord(
                audioLanguage: global.audioLanguage,
                subtitleLanguage: global.subtitleLanguage,
                subtitles: global.subtitles
            )
            let language = track.flatMap { $0.language.trimmingCharacters(in: .whitespaces).isEmpty ? nil : $0.language }
            if type == "audio" {
                preferences.audioLanguage = language
            } else {
                // Turning subtitles off keeps the language it was off *from*, so
                // turning them back on does not have to be told it again.
                preferences.subtitleLanguage = language ?? preferences.subtitleLanguage
                preferences.subtitles = track != nil
            }
            try? engine.setTitleTrackPreferences(titleKey: titleKey, preferences: preferences)
        }
    }

    func dismissProblem() {
        playbackError = nil
        player?.problem = nil
    }

    // MARK: - Background

    /// Leaving the screen: the picture goes, the sound stays if the viewer
    /// asked for background audio, and the position is saved either way.
    func enteredBackground() {
        save()
        guard isOpen, let player else { return }
        if SettingsStore().backgroundAudio {
            player.setVideoEnabled(false)
        } else {
            player.setPaused(true)
        }
        publishNowPlaying()
    }

    func enteredForeground() {
        player?.setVideoEnabled(true)
    }

    // MARK: - Lock screen

    /// What the lock screen and Control Center show: the episode, its show and
    /// its art — never just the app.
    private func publishNowPlaying() {
        guard let title, let episode else {
            MPNowPlayingInfoCenter.default().nowPlayingInfo = nil
            return
        }
        let center = MPNowPlayingInfoCenter.default()
        var info = center.nowPlayingInfo ?? [:]
        info[MPMediaItemPropertyTitle] = episode.label
        info[MPMediaItemPropertyArtist] = title.displayName
        info[MPMediaItemPropertyAlbumTitle] = title.displayName
        info[MPNowPlayingInfoPropertyMediaType] = MPNowPlayingInfoMediaType.video.rawValue
        info[MPMediaItemPropertyPlaybackDuration] = player?.duration ?? 0
        info[MPNowPlayingInfoPropertyElapsedPlaybackTime] = player?.position ?? 0
        info[MPNowPlayingInfoPropertyPlaybackRate] = (player?.paused ?? true) ? 0.0 : speed
        center.nowPlayingInfo = info
        let commands = MPRemoteCommandCenter.shared()
        commands.nextTrackCommand.isEnabled = next != nil
        commands.previousTrackCommand.isEnabled = index > 0
        UIApplication.shared.isIdleTimerDisabled = !(player?.paused ?? true) && !minimized

        let artKey = mediaKey(episode)
        if artworkFor != artKey {
            artworkFor = artKey
            info[MPMediaItemPropertyArtwork] = nil
            center.nowPlayingInfo = info
            let url = title.posterUrl ?? title.backdropUrl
            let source = episode.thumbnailSource
            Task {
                guard let image = await ArtworkLoader.shared.artwork(url: url, fallback: source), artworkFor == artKey else { return }
                var current = center.nowPlayingInfo ?? [:]
                current[MPMediaItemPropertyArtwork] = MPMediaItemArtwork(boundsSize: image.size) { _ in image }
                center.nowPlayingInfo = current
            }
        }
    }

    private func updateElapsed(_ seconds: Double) {
        let center = MPNowPlayingInfoCenter.default()
        guard var info = center.nowPlayingInfo else { return }
        info[MPNowPlayingInfoPropertyElapsedPlaybackTime] = seconds
        center.nowPlayingInfo = info
    }

    private func configureRemoteCommands() {
        let commands = MPRemoteCommandCenter.shared()
        commands.playCommand.addTarget { [weak self] _ in
            MainActor.assumeIsolated { self?.setPaused(false) }
            return .success
        }
        commands.pauseCommand.addTarget { [weak self] _ in
            MainActor.assumeIsolated { self?.setPaused(true) }
            return .success
        }
        commands.togglePlayPauseCommand.addTarget { [weak self] _ in
            MainActor.assumeIsolated { self?.setPaused(!(self?.player?.paused ?? true)) }
            return .success
        }
        commands.changePlaybackPositionCommand.addTarget { [weak self] event in
            guard let event = event as? MPChangePlaybackPositionCommandEvent else { return .commandFailed }
            MainActor.assumeIsolated { self?.seek(to: event.positionTime) }
            return .success
        }
        commands.nextTrackCommand.addTarget { [weak self] _ in
            MainActor.assumeIsolated { self?.advance() }
            return .success
        }
        commands.previousTrackCommand.addTarget { [weak self] _ in
            MainActor.assumeIsolated { self?.retreat() }
            return .success
        }
        commands.stopCommand.addTarget { [weak self] _ in
            MainActor.assumeIsolated { self?.close() }
            return .success
        }
    }

    /// A call or an alarm pauses, and its end resumes only what this paused —
    /// Android's transient focus loss. Headphones coming out pause, as
    /// `BECOMING_NOISY` does there.
    private func observeAudioSession() {
        let center = NotificationCenter.default
        center.addObserver(forName: AVAudioSession.interruptionNotification, object: nil, queue: .main) { [weak self] note in
            guard let raw = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
                  let type = AVAudioSession.InterruptionType(rawValue: raw)
            else { return }
            let options = (note.userInfo?[AVAudioSessionInterruptionOptionKey] as? UInt).map(AVAudioSession.InterruptionOptions.init)
            MainActor.assumeIsolated {
                guard let self, let player = self.player, self.isOpen else { return }
                switch type {
                case .began:
                    if !player.paused {
                        self.pausedByInterruption = true
                        self.setPaused(true)
                    }
                case .ended:
                    if self.pausedByInterruption, options?.contains(.shouldResume) == true { self.setPaused(false) }
                    self.pausedByInterruption = false
                @unknown default:
                    break
                }
            }
        }
        center.addObserver(forName: AVAudioSession.routeChangeNotification, object: nil, queue: .main) { [weak self] note in
            guard let raw = note.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt,
                  AVAudioSession.RouteChangeReason(rawValue: raw) == .oldDeviceUnavailable
            else { return }
            MainActor.assumeIsolated {
                guard let self, self.isOpen else { return }
                self.setPaused(true)
            }
        }
    }
}
