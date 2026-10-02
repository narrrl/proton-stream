import AVFoundation
import SwiftUI

@main
struct ProtonStreamApp: App {
    @State private var model = AppModel()

    init() {
        // `.playback` keeps the audio going with the screen locked or the app
        // in the background, which `UIBackgroundModes: audio` then permits.
        try? AVAudioSession.sharedInstance().setCategory(.playback, mode: .moviePlayback)
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .preferredColorScheme(.dark)
        }
    }
}

struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if let failure = model.startupError {
            ContentUnavailableView("Could not start", systemImage: "exclamationmark.triangle", description: Text(failure))
        } else {
            TabView {
                NavigationStack { LibraryView() }
                    .tabItem { Label("Library", systemImage: "play.rectangle.on.rectangle") }
                NavigationStack { SharesView() }
                    .tabItem { Label("Shares", systemImage: "link") }
                NavigationStack { SettingsView() }
                    .tabItem { Label("Settings", systemImage: "gearshape") }
            }
            .task { await model.reload() }
            .alert("Something went wrong", isPresented: .init(
                get: { model.error != nil },
                set: { if !$0 { model.error = nil } }
            )) {
                Button("OK", role: .cancel) {}
            } message: {
                Text(model.error ?? "")
            }
        }
    }
}
