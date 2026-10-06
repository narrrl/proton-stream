import SwiftUI
import UIKit

/// How long the controls stay up after a tap.
private let controlsSeconds = 4.0

/// The narrowest layout the in-app volume slider is worth the room it costs.
/// Below it — a phone held upright — the hardware volume buttons are the
/// better answer.
private let volumeSliderMinWidth: CGFloat = 600

/// The rates worth offering. A short list rather than a slider: the useful
/// rates are a handful of ratios, and a slider lands on 1.07 as often as 1.0.
private let speeds = [0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0]

/// The player, over the whole window, notch included.
///
/// The episode itself lives in `PlayerHost`, so this view can go — minimised to
/// the mini transport, or with the app in the background — while it plays on.
struct PlayerView: View {
    let host: PlayerHost
    let player: MpvPlayer
    @Environment(\.scheme) private var scheme
    @State private var showControls = true
    @State private var hideTask: Task<Void, Never>?
    @State private var landscapeLocked = OrientationLock.landscape
    @State private var sheet: PlayerSheet?

    var body: some View {
        GeometryReader { geometry in
            ZStack {
                Color.black.ignoresSafeArea()
                MpvView(player: player).ignoresSafeArea()
                Color.clear
                    .contentShape(Rectangle())
                    .ignoresSafeArea()
                    .onTapGesture { setControls(!showControls) }

                VStack(spacing: 0) {
                    if showControls { topBar.transition(.opacity) }
                    Spacer(minLength: 0)
                    if showControls {
                        PlayerControls(
                            host: host,
                            player: player,
                            roomForVolume: geometry.size.width >= volumeSliderMinWidth,
                            landscapeLocked: landscapeLocked,
                            onRotate: { landscapeLocked = OrientationLock.toggle() },
                            onSheet: { sheet = $0 },
                            onInteract: { setControls(true) }
                        )
                        .transition(.opacity)
                    }
                }

                // Drawn whether or not the rest of the controls are up: an
                // opening lasts ninety seconds, and a button you have to wake
                // with a tap is one nobody reaches in time.
                VStack(alignment: .trailing, spacing: 8) {
                    Spacer()
                    if let offer = host.offer, !host.autoSkip {
                        Button(offer.label) { host.seek(to: offer.target) }.buttonStyle(.tonal)
                    }
                    UpNextCard(
                        next: host.next,
                        visible: host.autoplay && !player.paused && host.chapters.creditsStart.map { player.position >= $0 } == true,
                        onPlayNow: host.advance
                    )
                }
                .frame(maxWidth: .infinity, alignment: .trailing)
                .padding(.trailing, 16)
                .padding(.bottom, showControls ? 132 : 24)

                // Only while nothing else explains the still picture: an error
                // is the more useful thing to say.
                if player.stalled && host.playbackError == nil {
                    VStack(spacing: 12) {
                        ProgressView().controlSize(.large).tint(scheme.primary)
                        if player.buffering {
                            Text(player.cachePercent > 0 ? "Buffering… \(Int(player.cachePercent.rounded()))%" : "Buffering…")
                                .textStyle(.labelMedium).foregroundStyle(scheme.onSurface)
                        }
                    }
                    .padding(20)
                    .background(scheme.surfaceContainerHighest, in: RoundedRectangle(cornerRadius: Corner.standard))
                }

                if let message = host.playbackError ?? player.problem {
                    VStack(spacing: 4) {
                        Text(message).foregroundStyle(scheme.onErrorContainer).multilineTextAlignment(.center)
                        // Dismissible, because the message is not always fatal
                        // — a subtitle track that failed to load leaves an
                        // episode that plays perfectly well behind it.
                        Button("Dismiss", action: host.dismissProblem)
                            .buttonStyle(.quiet)
                            .tint(scheme.onErrorContainer)
                    }
                    .textStyle(.bodyLarge)
                    .padding(16)
                    .background(scheme.errorContainer, in: RoundedRectangle(cornerRadius: Corner.standard))
                    .padding(32)
                }
            }
        }
        .animation(.easeInOut(duration: 0.2), value: showControls)
        .statusBarHidden(!showControls)
        .persistentSystemOverlays(.hidden)
        .onAppear { setControls(true) }
        .onChange(of: player.paused) { setControls(showControls) }
        .sheet(item: $sheet) { which in
            switch which {
            case .speed: SpeedSheet(current: host.speed, onSelect: host.setSpeed)
            case .episodes: EpisodeSheet(episodes: host.episodes, playing: host.index, onSelect: host.jump)
            case .chapters: ChapterSheet(chapters: host.chapters.entries, position: player.position) { host.seek(to: $0.start) }
            case let .tracks(type):
                TrackSheet(type: type, tracks: player.tracks.filter { $0.type == type }) { host.select(type: type, track: $0) }
            }
        }
    }

    private var topBar: some View {
        HStack(spacing: 0) {
            // Two ways out, because they mean different things: this leaves the
            // video and keeps the episode playing, ✕ ends it.
            IconButton("chevron.down", "Leave the video", tint: .white) {
                // The rest of the app is not a landscape app.
                landscapeLocked = false
                OrientationLock.set(false)
                host.minimized = true
            }
            // The episode leads and the show follows it: which episode is the
            // thing a viewer who has just woken the controls is checking.
            VStack(alignment: .leading, spacing: 0) {
                Text(host.episode?.label ?? "").textStyle(.titleMedium).foregroundStyle(.white).lineLimit(1)
                Text([host.title?.displayName, host.episode?.detail].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: "  •  "))
                    .textStyle(.bodySmall).foregroundStyle(.white.opacity(0.7)).lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            IconButton("xmark", "Stop playback", tint: .white, action: host.close)
            if player.duration > 0, player.duration - player.position >= 1 {
                Text("\(clock(player.duration - player.position)) left")
                    .textStyle(.labelMedium).foregroundStyle(.white.opacity(0.7))
                    .padding(.leading, 12).padding(.trailing, 4)
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background(scheme.scrim.opacity(0.45).ignoresSafeArea(edges: [.top, .horizontal]))
    }

    /// Show or hide the controls; shown and playing, they hide themselves.
    private func setControls(_ shown: Bool) {
        showControls = shown
        hideTask?.cancel()
        guard shown, !player.paused else { return }
        hideTask = Task {
            try? await Task.sleep(for: .seconds(controlsSeconds))
            if !Task.isCancelled { showControls = false }
        }
    }
}

enum PlayerSheet: Identifiable {
    case speed, episodes, chapters
    case tracks(String)

    var id: String {
        switch self {
        case .speed: "speed"
        case .episodes: "episodes"
        case .chapters: "chapters"
        case let .tracks(type): "tracks-\(type)"
        }
    }
}

/// Everything the transport offers, in two rows: the seek bar on its own,
/// then the transport cluster on the left, where a thumb already is, and the
/// pickers on the right.
private struct PlayerControls: View {
    let host: PlayerHost
    let player: MpvPlayer
    let roomForVolume: Bool
    let landscapeLocked: Bool
    let onRotate: () -> Void
    let onSheet: (PlayerSheet) -> Void
    let onInteract: () -> Void
    @Environment(\.scheme) private var scheme
    // Where the thumb is while a finger is on it: a seek per drag step is a
    // block fetch per drag step, and the thumb would snap back under the
    // finger between readings.
    @State private var scrubbing: Double?
    @State private var dragVolume: Double?

    var body: some View {
        let duration = max(player.duration, 1)
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Text(clock(scrubbing ?? player.position)).textStyle(.labelMedium).foregroundStyle(.white).monospacedDigit()
                AccentTrack(
                    value: Binding(
                        get: { scrubbing ?? min(max(player.position, 0), duration) },
                        set: { scrubbing = $0 }
                    ),
                    range: 0 ... duration,
                    buffered: player.duration > 0 ? player.cachedUntil / player.duration : 0,
                    ticks: player.duration > 0 && host.chapters.entries.count > 1
                        ? host.chapters.entries.dropFirst().map { $0.start / player.duration } : [],
                    onEditing: { editing in
                        onInteract()
                        if !editing, let target = scrubbing {
                            host.seek(to: target)
                            scrubbing = nil
                        }
                    }
                )
                Text(clock(player.duration)).textStyle(.labelMedium).foregroundStyle(.white).monospacedDigit()
            }
            HStack(spacing: 0) {
                CompactIcon("backward.end.fill", "Previous episode", enabled: host.index > 0, action: host.retreat)
                CompactIcon("gobackward.10", "Back ten seconds") { host.seek(to: player.position - 10) }
                Button { host.setPaused(!player.paused); onInteract() } label: {
                    Image(systemName: player.paused ? "play.fill" : "pause.fill")
                        .font(.system(size: 30))
                        .foregroundStyle(.white)
                        .frame(width: 48, height: 48)
                }
                .accessibilityLabel(player.paused ? "Play" : "Pause")
                CompactIcon("goforward.30", "Forward thirty seconds") { host.seek(to: player.position + 30) }
                CompactIcon("forward.end.fill", "Next episode", enabled: host.next != nil, action: host.advance)
                Spacer(minLength: 4)
                if host.episodes.count > 1 {
                    CompactIcon("rectangle.stack", "Episodes") { onSheet(.episodes) }
                }
                if host.chapters.entries.count > 1 {
                    CompactIcon("list.bullet", "Chapters") { onSheet(.chapters) }
                }
                CompactIcon("speedometer", "Playback speed") { onSheet(.speed) }
                CompactIcon("waveform", "Audio track") { onSheet(.tracks("audio")) }
                CompactIcon("captions.bubble", "Subtitles") { onSheet(.tracks("sub")) }
                CompactIcon(player.muted ? "speaker.slash.fill" : "speaker.wave.2.fill", "Mute") {
                    player.setMuted(!player.muted)
                    host.rememberVolume(muted: !player.muted)
                }
                // The app's own volume, which is not the device's: a file
                // mastered quiet is turned up here and stays up for the next.
                if roomForVolume {
                    Slider(
                        value: Binding(
                            get: { dragVolume ?? min(max(player.volume, 0), 100) },
                            set: { dragVolume = $0; player.setVolume($0) }
                        ),
                        in: 0 ... 100
                    ) { editing in
                        onInteract()
                        if !editing, let chosen = dragVolume {
                            dragVolume = nil
                            host.rememberVolume(volume: chosen)
                        }
                    }
                    .tint(scheme.primary)
                    .frame(width: 96)
                }
                CompactIcon(
                    landscapeLocked ? "arrow.down.right.and.arrow.up.left" : "arrow.up.left.and.arrow.down.right",
                    landscapeLocked ? "Unlock rotation" : "Lock to landscape",
                    action: onRotate
                )
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 4)
        .background(scheme.scrim.opacity(0.68).ignoresSafeArea(edges: [.bottom, .horizontal]))
    }
}

private struct CompactIcon: View {
    let icon: String
    let label: String
    var enabled = true
    let action: () -> Void

    init(_ icon: String, _ label: String, enabled: Bool = true, action: @escaping () -> Void) {
        self.icon = icon
        self.label = label
        self.enabled = enabled
        self.action = action
    }

    var body: some View {
        Button(action: action) {
            Image(systemName: icon)
                .font(.system(size: 19))
                .foregroundStyle(enabled ? Color.white : Color.white.opacity(0.35))
                .frame(width: 40, height: 40)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        .accessibilityLabel(label)
    }
}

/// The countdown into the next episode, shown once the credits run starts.
///
/// "Watch till the end" holds for the rest of the file: someone who wants the
/// post-credits scene should have to say so once, not once every ten seconds.
private struct UpNextCard: View {
    let next: EpisodeRecord?
    let visible: Bool
    let onPlayNow: () -> Void
    @Environment(\.scheme) private var scheme
    @State private var held = false
    @State private var remaining = upNextSeconds
    @State private var heldFor: String?

    var body: some View {
        let showing = visible && next != nil && !(held && heldFor == next?.linkId)
        Group {
            if showing, let upNext = next {
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 12) {
                        // The next episode's still, so the card says which
                        // episode without being read; the ring over it is both
                        // the countdown and the button that skips it.
                        Button(action: onPlayNow) {
                            RemoteArtwork(upNext.stillUrl, upNext.label, fallback: upNext.thumbnailSource, labelled: false)
                                .frame(width: 112)
                                .aspectRatio(16 / 9, contentMode: .fit)
                                .overlay {
                                    ZStack {
                                        Circle().fill(.black.opacity(0.5))
                                        Circle().stroke(.white.opacity(0.25), lineWidth: 3)
                                        Circle()
                                            .trim(from: 0, to: Double(remaining) / Double(upNextSeconds))
                                            .stroke(.white, style: StrokeStyle(lineWidth: 3, lineCap: .round))
                                            .rotationEffect(.degrees(-90))
                                            .animation(.linear(duration: 1), value: remaining)
                                        Image(systemName: "play.fill").foregroundStyle(.white)
                                    }
                                    .frame(width: 40, height: 40)
                                }
                                .clipShape(RoundedRectangle(cornerRadius: Corner.standard))
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Play now")
                        VStack(alignment: .leading, spacing: 2) {
                            Text("Up next in \(remaining)s").textStyle(.labelMedium).foregroundStyle(scheme.onSurfaceVariant)
                            Text(upNext.providerName ?? upNext.label).textStyle(.titleSmall).foregroundStyle(scheme.onSurface).lineLimit(2)
                        }
                    }
                    Button("Watch till the end") {
                        held = true
                        heldFor = upNext.linkId
                    }
                    .buttonStyle(.quiet)
                }
                .padding(12)
                .frame(width: 280)
                .background(scheme.surfaceContainerHighest, in: RoundedRectangle(cornerRadius: Corner.standard))
                .task(id: upNext.linkId) {
                    remaining = upNextSeconds
                    while remaining > 0 {
                        try? await Task.sleep(for: .seconds(1))
                        if Task.isCancelled { return }
                        remaining -= 1
                    }
                    onPlayNow()
                }
            }
        }
    }
}

private struct SpeedSheet: View {
    let current: Double
    let onSelect: (Double) -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        DialogSheet("Playback speed") {
            ForEach(speeds, id: \.self) { rate in
                Button {
                    onSelect(rate)
                    dismiss()
                } label: {
                    Text((abs(rate - current) < 0.001 ? "✓  " : "") + (rate == 1 ? "Normal" : "\(rate.formatted())×"))
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.edgedWide)
            }
        } buttons: {
            Button("Close") { dismiss() }.buttonStyle(.tonal)
        }
    }
}

private struct ChapterSheet: View {
    let chapters: [ChapterEntry]
    let position: Double
    let onSelect: (ChapterEntry) -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        let current = chapters.lastIndex { position + 0.001 >= $0.start }
        DialogSheet("Chapters") {
            ForEach(Array(chapters.enumerated()), id: \.element.index) { position, chapter in
                Button {
                    onSelect(chapter)
                    dismiss()
                } label: {
                    Text((position == current ? "✓  " : "") + "\(clock(chapter.start))  \(chapter.label)")
                        .lineLimit(1)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .buttonStyle(.edgedWide)
            }
        } buttons: {
            Button("Close") { dismiss() }.buttonStyle(.tonal)
        }
    }
}

private struct TrackSheet: View {
    let type: String
    let tracks: [MpvTrack]
    let onSelect: (MpvTrack?) -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        DialogSheet(type == "audio" ? "Audio track" : "Subtitle track") {
            if type == "sub" {
                Button {
                    onSelect(nil)
                    dismiss()
                } label: {
                    Text(tracks.contains(where: \.selected) ? "Off" : "✓  Off").frame(maxWidth: .infinity, alignment: .leading)
                }
                .buttonStyle(.edgedWide)
            }
            ForEach(tracks) { track in
                Button {
                    onSelect(track)
                    dismiss()
                } label: {
                    Text(track.selected ? "✓  \(track.label)" : track.label).frame(maxWidth: .infinity, alignment: .leading)
                }
                .buttonStyle(.edgedWide)
            }
        } buttons: {
            Button("Close") { dismiss() }.buttonStyle(.tonal)
        }
    }
}

/// The title's episodes, from the player, opened scrolled to the one playing
/// so the next few are in view. Each row has its still, its name, and whether
/// it is playing, watched or part way.
private struct EpisodeSheet: View {
    let episodes: [EpisodeRecord]
    let playing: Int
    let onSelect: (Int) -> Void
    @Environment(\.dismiss) private var dismiss
    @Environment(\.scheme) private var scheme

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Episodes").textStyle(.titleMedium).foregroundStyle(scheme.onSurface)
                .padding(.horizontal, 24).padding(.vertical, 8)
            ScrollViewReader { reader in
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(Array(episodes.enumerated()), id: \.element.linkId) { position, episode in
                            row(position, episode).id(position)
                        }
                    }
                    .padding(.bottom, 24)
                }
                .onAppear { reader.scrollTo(max(playing - 1, 0), anchor: .top) }
            }
        }
        .padding(.top, 20)
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .presentationBackground(scheme.surfaceContainerLow)
        .presentationCornerRadius(Corner.sheet)
    }

    private func row(_ position: Int, _ episode: EpisodeRecord) -> some View {
        let current = position == playing
        return Button {
            dismiss()
            onSelect(position)
        } label: {
            HStack(spacing: 12) {
                RemoteArtwork(episode.stillUrl, episode.label, fallback: episode.thumbnailSource, labelled: false)
                    .frame(width: 112)
                    .aspectRatio(16 / 9, contentMode: .fit)
                    .overlay(alignment: .bottom) {
                        if let progress = episode.progress, !episode.watched, !current { AccentProgress(progress: progress) }
                    }
                    .clipShape(RoundedRectangle(cornerRadius: Corner.standard))
                VStack(alignment: .leading, spacing: 2) {
                    Text(episode.providerName ?? episode.label).textStyle(.titleSmall).foregroundStyle(scheme.onSurface).lineLimit(2)
                    Text(current ? "Playing"
                        : episode.watched ? "\(episode.label) · Watched"
                        : episode.resumeAt != nil ? "\(episode.label) · Part way" : episode.label)
                        .textStyle(.bodySmall)
                        .foregroundStyle(current ? scheme.primary : scheme.onSurfaceVariant)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 8)
            .background(current ? scheme.secondaryContainer : .clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// The bar that says playback is still going, and gets back to it.
///
/// Leaving the player does not stop it — the episode keeps playing — so there
/// has to be something on screen that says so and takes one tap to return to.
struct MiniTransport: View {
    let host: PlayerHost
    @Environment(\.scheme) private var scheme

    var body: some View {
        if let episode = host.episode, let title = host.title {
            HStack(spacing: 0) {
                RemoteArtwork(title.posterUrl ?? title.backdropUrl, title.displayName, fallback: episode.thumbnailSource)
                    .frame(width: 64)
                    .aspectRatio(16 / 9, contentMode: .fit)
                    .clipShape(RoundedRectangle(cornerRadius: Corner.standard))
                VStack(alignment: .leading, spacing: 0) {
                    Text(episode.label).textStyle(.bodyLarge, semibold: true).foregroundStyle(scheme.onSurface).lineLimit(1)
                    Text(title.displayName).textStyle(.bodySmall).foregroundStyle(scheme.onSurfaceVariant).lineLimit(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 12)
                let paused = host.player?.paused ?? true
                IconButton(paused ? "play.fill" : "pause.fill", paused ? "Play" : "Pause", tint: scheme.onSurface) {
                    host.setPaused(!paused)
                }
                IconButton("xmark", "Stop playback", tint: scheme.onSurface, action: host.close)
            }
            .padding(8)
            .card()
            .shadow(color: .black.opacity(0.25), radius: 8, y: 2)
            .padding(.horizontal, 8)
            .contentShape(Rectangle())
            .onTapGesture { host.minimized = false }
        }
    }
}

/// The view libmpv draws into: its Metal layer, sized to this view.
struct MpvView: UIViewRepresentable {
    let player: MpvPlayer

    func makeUIView(context _: Context) -> LayerView {
        LayerView(layer: player.layer)
    }

    func updateUIView(_: LayerView, context _: Context) {}

    final class LayerView: UIView {
        private let video: CALayer

        init(layer video: CALayer) {
            self.video = video
            super.init(frame: .zero)
            backgroundColor = .black
            video.removeFromSuperlayer()
            layer.addSublayer(video)
        }

        @available(*, unavailable)
        required init?(coder _: NSCoder) { nil }

        override func layoutSubviews() {
            super.layoutSubviews()
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            video.frame = bounds
            video.contentsScale = window?.screen.scale ?? UIScreen.main.scale
            CATransaction.commit()
        }
    }
}

/// Lock to landscape, or hand rotation back to the device.
@MainActor
enum OrientationLock {
    private(set) static var landscape = false

    /// What the app delegate answers for every window.
    static var mask: UIInterfaceOrientationMask { landscape ? .landscape : .all }

    /// Reports which way it went.
    static func toggle() -> Bool {
        set(!landscape)
        return landscape
    }

    static func set(_ locked: Bool) {
        landscape = locked
        guard let scene = UIApplication.shared.connectedScenes.first(where: { $0 is UIWindowScene }) as? UIWindowScene else { return }
        scene.keyWindow?.rootViewController?.setNeedsUpdateOfSupportedInterfaceOrientations()
        scene.requestGeometryUpdate(.iOS(interfaceOrientations: mask))
    }
}
