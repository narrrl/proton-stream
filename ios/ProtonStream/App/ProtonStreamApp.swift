import AVFoundation
import SwiftUI
import UIKit

@main
struct ProtonStreamApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var model = AppModel()
    @State private var appearance = AppearanceState.shared

    init() {
        // `.playback` keeps the audio going with the screen locked or the app
        // in the background, which `UIBackgroundModes: audio` then permits.
        try? AVAudioSession.sharedInstance().setCategory(.playback, mode: .moviePlayback)
        // The palette read before the first frame, so the first frame is
        // already painted in it.
        AppearanceState.shared.seed(NativeRuntime.initialPalette())
    }

    var body: some Scene {
        WindowGroup {
            let scheme = appearance.scheme
            RootView()
                .environment(model)
                .environment(\.scheme, scheme)
                .preferredColorScheme(scheme.colorScheme)
                .tint(scheme.primary)
                .font(.inter(.bodyLarge))
                .background(scheme.background.ignoresSafeArea())
                .task { await appearance.load() }
        }
    }
}

final class AppDelegate: NSObject, UIApplicationDelegate {
    func application(_: UIApplication, supportedInterfaceOrientationsFor _: UIWindow?) -> UIInterfaceOrientationMask {
        MainActor.assumeIsolated { OrientationLock.mask }
    }
}

/// From this width the library and an open title share the window.
private let twoPaneWidth: CGFloat = 840

/// From this width the destinations are a rail down the side, not a bar.
private let railWidth: CGFloat = 600

/// How much room the floating mini transport needs at the foot of a page.
private let miniTransportInset: CGFloat = 88

enum Destination: String, CaseIterable {
    case library, history, shares, downloads, settings

    var label: String {
        switch self {
        case .library: "Library"
        case .history: "History"
        case .shares: "Shares"
        case .downloads: "Downloads"
        case .settings: "Settings"
        }
    }

    var icon: String {
        switch self {
        case .library: "play.rectangle.on.rectangle.fill"
        case .history: "clock.arrow.circlepath"
        case .shares: "square.and.arrow.up"
        case .downloads: "arrow.down.circle"
        case .settings: "gearshape"
        }
    }
}

struct RootView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scheme) private var scheme
    @Environment(\.scenePhase) private var phase
    // Keys, not records: what has to survive is *which* title, and a record is
    // no longer current after the library reloads. Everything else is derived,
    // so a reload that renames a season updates the open screen.
    @SceneStorage("destination") private var destination: Destination = .library
    @SceneStorage("selectedTitle") private var selectedTitleKey: String?
    @State private var incomingLink: String?
    @State private var host = PlayerHost.shared

    var body: some View {
        GeometryReader { geometry in
            let wide = geometry.size.width >= railWidth
            ZStack(alignment: .bottom) {
                HStack(spacing: 0) {
                    if wide { NavigationRail(destination: $destination, onSelect: select) }
                    page(width: geometry.size.width)
                        .safeAreaInset(edge: .bottom, spacing: 0) {
                            VStack(spacing: 8) {
                                if host.isOpen && host.minimized {
                                    MiniTransport(host: host)
                                }
                                if !wide { NavigationBar(destination: $destination, onSelect: select) }
                            }
                        }
                }
                VStack(spacing: 8) {
                    ActivityBanner()
                    Snackbar()
                }
                .padding(.bottom, (wide ? 16 : 72) + (host.isOpen && host.minimized ? miniTransportInset : 0))
                .animation(.easeInOut(duration: 0.2), value: model.activity)
            }
            .background(scheme.background.ignoresSafeArea())
        }
        .overlay {
            // Over the shell rather than instead of it: the page behind stays
            // where it was, so leaving the player finds the title page as it was.
            if host.isOpen && !host.minimized {
                Group {
                    if let player = host.player {
                        PlayerView(host: host, player: player)
                    } else {
                        ZStack(alignment: .topTrailing) {
                            Color.black.ignoresSafeArea()
                            Text(host.playbackError ?? "Playback is unavailable: libmpv did not start.")
                                .foregroundStyle(.white)
                                .frame(maxWidth: .infinity, maxHeight: .infinity)
                            IconButton("xmark", "Close", tint: .white, action: host.close)
                        }
                    }
                }
                .transition(.move(edge: .bottom))
            }
        }
        .animation(.easeInOut(duration: 0.25), value: host.isOpen && !host.minimized)
        .onAppear {
            model.start()
            host.onSaveProgress = { [model] episode, position, duration, watched in
                model.saveProgress(episode, position: position, duration: duration, watched: watched)
            }
            host.onClosed = { [model] in model.playerClosed() }
        }
        .onChange(of: model.titles) { _, titles in host.update(titles: titles) }
        .onChange(of: phase) { _, phase in
            switch phase {
            case .background:
                host.enteredBackground()
                model.syncInBackground()
            case .active:
                host.enteredForeground()
                model.enteredForeground()
                DownloadCoordinator.shared.resumeQueued()
            default:
                break
            }
        }
        // A link opened in this app: show the Add form with it filled in. A
        // playing episode keeps going in the mini transport rather than
        // covering the form.
        .onOpenURL { url in
            guard let link = shareLink(from: url) else { return }
            incomingLink = link
            destination = .shares
            selectedTitleKey = nil
            if host.isOpen { host.minimized = true }
        }
    }

    private func select(_ target: Destination) {
        destination = target
        selectedTitleKey = nil
    }

    private func play(_ title: TitleRecord, _ index: Int) {
        host.play(title, at: index)
    }

    @ViewBuilder private func page(width: CGFloat) -> some View {
        switch destination {
        case .library:
            libraryPage(width: width)
        case .history:
            HistoryView(onPlay: play, onTitle: { title in
                destination = .library
                selectedTitleKey = title.key
            })
        case .shares:
            SharesView(incomingLink: $incomingLink)
        case .downloads:
            DownloadsView()
        case .settings:
            SettingsView()
        }
    }

    /// Side by side where there is room for both: on a tablet the library
    /// stays in view while a title is open, so moving between titles is one
    /// tap rather than back and in again.
    @ViewBuilder private func libraryPage(width: CGFloat) -> some View {
        let selected = selectedTitleKey.flatMap { key in model.titles.first { $0.key == key } }
        let library = LibraryView(onResume: play, onTitle: { title in withAnimation(.easeOut(duration: 0.25)) { selectedTitleKey = title.key } })
        if width >= twoPaneWidth {
            HStack(spacing: 0) {
                library.frame(width: width * 0.45)
                Rectangle().fill(scheme.outlineVariant).frame(width: 1).ignoresSafeArea()
                Group {
                    if let selected { titlePane(selected) } else { EmptyState("Choose a title", "Its seasons and episodes open here.") }
                }
                .frame(maxWidth: .infinity)
            }
        } else {
            // The library stays underneath the title page rather than being
            // rebuilt, so going back finds the grid scrolled where it was.
            ZStack {
                library
                    .opacity(selected == nil ? 1 : 0)
                    .allowsHitTesting(selected == nil)
                    .accessibilityHidden(selected != nil)
                if let selected {
                    titlePane(selected)
                        .transition(.asymmetric(insertion: .move(edge: .trailing), removal: .opacity.combined(with: .scale(scale: 0.9))))
                        .zIndex(1)
                }
            }
        }
    }

    private func titlePane(_ selected: TitleRecord) -> some View {
        TitleView(
            title: selected,
            onPlay: { _, index in host.play(selected, at: index) },
            onBack: { withAnimation(.easeOut(duration: 0.25)) { selectedTitleKey = nil } },
            franchise: selected.franchise.compactMap { key in model.titles.first { $0.key == key } },
            onOpenTitle: { selectedTitleKey = $0.key }
        )
        .id(selected.key)
    }
}

/// The destinations along the foot of a phone, the selected one in an accent
/// pill — the desktop's rule is that the accent marks the thing you are on.
private struct NavigationBar: View {
    @Binding var destination: Destination
    let onSelect: (Destination) -> Void
    @Environment(\.scheme) private var scheme

    var body: some View {
        HStack(spacing: 0) {
            ForEach(Destination.allCases, id: \.self) { item in
                let selected = item == destination
                Button { onSelect(item) } label: {
                    VStack(spacing: 4) {
                        Image(systemName: item.icon)
                            .font(.system(size: 18))
                            .foregroundStyle(selected ? scheme.onPrimary : scheme.onSurfaceVariant)
                            .frame(width: 56, height: 30)
                            .background(selected ? scheme.primary : .clear, in: Capsule())
                        Text(item.label)
                            .textStyle(.labelMedium, semibold: selected)
                            .foregroundStyle(selected ? scheme.onSurface : scheme.onSurfaceVariant)
                            .lineLimit(1)
                            .minimumScaleFactor(0.8)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.top, 10)
                    .padding(.bottom, 6)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(item.label)
                .accessibilityAddTraits(selected ? .isSelected : [])
            }
        }
        .background(scheme.surfaceContainer.ignoresSafeArea(edges: .bottom))
    }
}

/// The same destinations down the side of a wide window.
private struct NavigationRail: View {
    @Binding var destination: Destination
    let onSelect: (Destination) -> Void
    @Environment(\.scheme) private var scheme

    var body: some View {
        VStack(spacing: 12) {
            ForEach(Destination.allCases, id: \.self) { item in
                let selected = item == destination
                Button { onSelect(item) } label: {
                    VStack(spacing: 4) {
                        Image(systemName: item.icon)
                            .font(.system(size: 18))
                            .foregroundStyle(selected ? scheme.onPrimary : scheme.onSurfaceVariant)
                            .frame(width: 56, height: 32)
                            .background(selected ? scheme.primary : .clear, in: Capsule())
                        Text(item.label)
                            .textStyle(.labelMedium, semibold: selected)
                            .foregroundStyle(selected ? scheme.onSurface : scheme.onSurfaceVariant)
                    }
                    .frame(width: 80)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(item.label)
                .accessibilityAddTraits(selected ? .isSelected : [])
            }
            Spacer()
        }
        .padding(.top, 24)
        .background(scheme.surface.ignoresSafeArea())
    }
}

/// What the app is busy with, for as long as it is: a spinner and one line.
/// Not dismissible and not timed — it goes when the work does.
private struct ActivityBanner: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scheme) private var scheme

    var body: some View {
        if let activity = model.activity {
            HStack(spacing: 12) {
                ProgressView().controlSize(.small).tint(scheme.primary)
                Text(activity)
                    .textStyle(.bodyMedium)
                    .foregroundStyle(scheme.onSurface)
                    .lineLimit(2)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
            .background(scheme.surfaceContainerHighest, in: RoundedRectangle(cornerRadius: Corner.tight))
            .padding(.horizontal, 12)
            .transition(.move(edge: .bottom).combined(with: .opacity))
            .accessibilityElement(children: .combine)
            .accessibilityAddTraits(.updatesFrequently)
        }
    }
}

/// The app's one line of feedback: what just happened, with Undo where it can
/// be taken back.
private struct Snackbar: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scheme) private var scheme

    var body: some View {
        if let message = model.message {
            HStack(spacing: 8) {
                Text(message)
                    .textStyle(.bodyMedium)
                    .foregroundStyle(scheme.inverseOnSurface)
                    .frame(maxWidth: .infinity, alignment: .leading)
                if model.undo != nil {
                    Button("Undo", action: model.performUndo)
                        .textStyle(.labelLarge)
                        .foregroundStyle(scheme.inversePrimary)
                        .buttonStyle(.plain)
                        .padding(.horizontal, 8)
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 14)
            .background(scheme.inverseSurface, in: RoundedRectangle(cornerRadius: Corner.tight))
            .padding(.horizontal, 12)
            .transition(.move(edge: .bottom).combined(with: .opacity))
            .onTapGesture(perform: model.dismissMessage)
            // Long with an Undo to reach for, short without — Material's two
            // snackbar durations.
            .task(id: message) {
                try? await Task.sleep(for: .seconds(model.undo != nil ? 10 : 4))
                if !Task.isCancelled, model.message == message { model.dismissMessage() }
            }
        }
    }
}
