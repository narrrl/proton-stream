import SwiftUI

/// The areas the first page lists, each a page of its own.
enum SettingsPage: String, CaseIterable, Identifiable {
    case playback, storage, appearance, metadata, about

    var id: String { rawValue }

    var title: String {
        switch self {
        case .playback: "Playback"
        case .storage: "Downloads and storage"
        case .appearance: "Appearance"
        case .metadata: "Metadata"
        case .about: "About"
        }
    }

    var icon: String {
        switch self {
        case .playback: "play.circle"
        case .storage: "externaldrive"
        case .appearance: "paintpalette"
        case .metadata: "text.magnifyingglass"
        case .about: "info.circle"
        }
    }
}

/// The installed version, as the Home Screen and the sideloading store show it.
let appVersion = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "unknown"

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scheme) private var scheme
    // One row per area on the first page, each opening its own page.
    @SceneStorage("settings.page") private var page: SettingsPage?
    @State private var settings = SettingsValues()
    // Playback preferences are the shared store's, not this app's: they are the
    // same file the desktop client reads, so a language chosen on one is the
    // language the other starts in.
    @State private var prefs: PlaybackPrefsRecord?
    @State private var confirmDelete = false
    @State private var showMetadata = false
    @State private var legalDocument: LegalDocument?

    var body: some View {
        Group {
            if let page {
                subPage(page)
            } else {
                TabPage("Settings") {
                    ScrollView {
                        VStack(spacing: 0) {
                            ForEach(SettingsPage.allCases) { entry in
                                Button { page = entry } label: {
                                    ListRow(icon: entry.icon, headline: entry.title, supporting: subtitle(entry))
                                }
                                .buttonStyle(.plain)
                            }
                        }
                    }
                }
            }
        }
        // Read again whenever the list is shown, not once: sync can rewrite
        // them while the page is open, and a page holding the old copy would
        // write it back over the new one on the next switch.
        .task(id: page) {
            do {
                prefs = try await model.run { try $0.playbackPrefs() }
            } catch {
                model.reportError(error)
            }
        }
        .alert("Delete all offline episodes?", isPresented: $confirmDelete) {
            Button("Delete", role: .destructive, action: model.removeAllOffline)
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("\(model.storage.offlineCount) episodes (\(formatBytes(model.storage.offlineBytes))) will be removed from this device. Watch history is kept, and anything deleted can be downloaded again.")
        }
        .sheet(item: $legalDocument) { LegalDocumentSheet(document: $0) }
        .sheet(isPresented: $showMetadata) {
            MetadataSettingsSheet(current: model.metadataSettings) { enabled, provider, language, key in
                await model.saveMetadataSettings(enabled: enabled, provider: provider, language: language, apiKey: key)
            }
        }
    }

    private func subtitle(_ entry: SettingsPage) -> String {
        switch entry {
        case .playback: "Autoplay, languages and decoding"
        case .storage: "\(formatBytes(model.storage.cacheBytes)) cached · \(model.storage.offlineCount) offline"
        case .appearance: "Palette, accent and gradient"
        case .metadata: model.metadataSettings.enabled ? "On · \(model.metadataSettings.provider.displayName)" : "Off"
        case .about: "Version \(appVersion) · licences"
        }
    }

    /// One area's page: its name, a way back, and its rows.
    private func subPage(_ page: SettingsPage) -> some View {
        TabPage(page.title, leading: {
            IconButton("chevron.backward", "Back to settings", tint: scheme.onSurface) { self.page = nil }
        }) {
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    switch page {
                    case .playback: playback
                    case .storage: storage
                    case .appearance: AppearancePicker { model.reloadAfterMetadataChange() }.padding(.horizontal, 16)
                    case .metadata: metadata
                    case .about: about
                    }
                }
                .padding(.bottom, 24)
            }
        }
        // Back by an edge swipe, as from a title page.
        .gesture(
            DragGesture(minimumDistance: 20)
                .onEnded { drag in
                    if drag.startLocation.x < 24, drag.translation.width > 80 { self.page = nil }
                }
        )
    }

    @ViewBuilder private var playback: some View {
        if let current = prefs {
            SwitchRow(headline: "Autoplay next episode", supporting: "Starts the next episode when the credits begin",
                      isOn: preference(current.autoplayNext) { $0.autoplayNext = $1 })
            // Off is the safer default: a mis-named chapter then costs a tap on
            // Skip rather than a scene.
            SwitchRow(headline: "Skip openings and endings", supporting: "Uses the release's chapters; off shows a Skip button instead",
                      isOn: preference(current.autoSkip) { $0.autoSkip = $1 })
            SwitchRow(headline: "Subtitles", supporting: "Shown by default where a file has them",
                      isOn: preference(current.subtitles) { $0.subtitles = $1 })
            // Preferences, not filters: a file with nothing in the language
            // plays its own default track. A show that has been given its own
            // choice keeps it.
            LanguageSetting(headline: "Audio language", value: current.audioLanguage) { tag in
                update { $0.audioLanguage = tag }
            }
            LanguageSetting(headline: "Subtitle language", value: current.subtitleLanguage) { tag in
                update { $0.subtitleLanguage = tag }
            }
        }
        SwitchRow(headline: "Hardware decoding", supporting: "Turn off if video shows green or torn frames", isOn: $settings.hardwareDecoding)
        SwitchRow(headline: "Background audio", supporting: "Keeps playing with the app closed, from the lock screen", isOn: $settings.backgroundAudio)
    }

    @ViewBuilder private var storage: some View {
        let budget = settings.cacheBudgetGib
        SwitchRow(headline: "Download on Wi-Fi only", supporting: "Mobile data is never used for downloads", isOn: Binding(
            get: { settings.wifiOnly },
            set: {
                settings.wifiOnly = $0
                // A queue that already exists keeps the policy it was queued
                // under until it is re-issued.
                DownloadCoordinator.shared.applyNetworkPolicy()
            }
        ))
        VStack(alignment: .leading, spacing: 0) {
            Text("Streaming cache").textStyle(.bodyLarge).foregroundStyle(scheme.onSurface)
            Text("\(formatBytes(model.storage.cacheBytes)) of \(budget) GiB used").textStyle(.bodySmall).foregroundStyle(scheme.onSurfaceVariant)
            AccentProgress(progress: Double(model.storage.cacheBytes) / Double(SettingsStore.gibToBytes(budget)))
                .padding(.vertical, 8)
            // What has been watched is kept up to this much, so going back over
            // a scene costs no download. A lower choice frees the difference at
            // once.
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    ForEach(SettingsStore.cacheBudgetChoices, id: \.self) { gib in
                        FilterChip(label: "\(gib) GiB", selected: gib == budget) {
                            settings.cacheBudgetGib = gib
                            Task.detached { try? NativeRuntime.blockingEngine().setStreamCacheBudget(bytes: SettingsStore.gibToBytes(gib)) }
                        }
                    }
                }
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        // The cache is rebuildable, so it goes without asking. Offline episodes
        // are a choice the viewer made, so that one asks.
        SettingAction(headline: "Clear streaming cache", supporting: "Frees \(formatBytes(model.storage.cacheBytes))", action: model.clearBlockCache)
        let storage = model.storage
        SettingAction(
            headline: "Delete all offline episodes",
            supporting: "\(storage.offlineCount) \(storage.offlineCount == 1 ? "episode" : "episodes") · \(formatBytes(storage.offlineBytes))"
                + (storage.partialBytes > 0 ? " · \(formatBytes(storage.partialBytes)) unfinished" : ""),
            enabled: storage.offlineCount > 0 || storage.partialBytes > 0
        ) { confirmDelete = true }
    }

    @ViewBuilder private var metadata: some View {
        SettingAction(
            headline: "Metadata enrichment",
            supporting: model.metadataSettings.enabled
                ? "On · \(model.metadataSettings.provider.displayName) · sends title names to it"
                : "Off, the privacy default"
        ) { showMetadata = true }
        // Every title looked up again, matched ones included: the way out of a
        // library the provider answered wrong.
        SettingAction(
            headline: "Match everything again",
            supporting: "Looks every title up again, hand-picked matches excepted",
            enabled: model.metadataSettings.enabled && !model.refreshing
        ) { model.matchTitles(force: true) }
    }

    @ViewBuilder private var about: some View {
        ListRow(headline: "Proton Stream", supporting: "Version \(appVersion)")
        SettingAction(headline: "Licence", supporting: "GPL-3.0-or-later. Comes with absolutely no warranty.") {
            legalDocument = LegalDocument(title: "GNU GPL v3", resource: "GPL-3.0", type: "txt")
        }
        SettingAction(headline: "Third-party notices", supporting: "libmpv, Inter and the other components bundled") {
            legalDocument = LegalDocument(title: "Third-party notices", resource: "THIRD_PARTY_NOTICES", type: "md")
        }
    }

    private func preference(_ value: Bool, _ change: @escaping (inout PlaybackPrefsRecord, Bool) -> Void) -> Binding<Bool> {
        Binding(get: { value }, set: { on in update { change(&$0, on) } })
    }

    /// Change a preference on screen at once, and store it. A store that
    /// fails says so and puts the switch back, rather than leaving one that
    /// looks saved and is not.
    private func update(_ change: (inout PlaybackPrefsRecord) -> Void) {
        guard var next = prefs else { return }
        let before = next
        change(&next)
        prefs = next
        let stored = next
        Task {
            do {
                try await model.run { try $0.setPlaybackPrefs(prefs: stored) }
            } catch {
                if prefs == stored { prefs = before }
                model.reportError(error)
            }
        }
    }
}

/// `SettingsStore` as observable state, so a switch redraws when it is flipped.
@Observable
private final class SettingsValues {
    private let store = SettingsStore()
    var wifiOnly: Bool { didSet { store.wifiOnly = wifiOnly } }
    var backgroundAudio: Bool { didSet { store.backgroundAudio = backgroundAudio } }
    var hardwareDecoding: Bool { didSet { store.hardwareDecoding = hardwareDecoding } }
    var cacheBudgetGib: Int { didSet { store.cacheBudgetGib = cacheBudgetGib } }

    init() {
        wifiOnly = store.wifiOnly
        backgroundAudio = store.backgroundAudio
        hardwareDecoding = store.hardwareDecoding
        cacheBudgetGib = store.cacheBudgetGib
    }
}

/// A row that does something when tapped: opens a dialog, clears a cache.
private struct SettingAction: View {
    let headline: String
    let supporting: String
    var enabled = true
    let action: () -> Void

    var body: some View {
        Button(action: action) { ListRow(headline: headline, supporting: supporting, enabled: enabled) }
            .buttonStyle(.plain)
            .disabled(!enabled)
    }
}

private struct MetadataSettingsSheet: View {
    let current: MetadataSettingsRecord
    /// Resolves to why the settings were not stored, or nil once they are.
    let onSave: (Bool, MetadataProvider, String, String) async -> String?
    @Environment(\.dismiss) private var dismiss
    @Environment(\.scheme) private var scheme
    @State private var enabled: Bool
    @State private var provider: MetadataProvider
    @State private var language: String
    @State private var apiKey = ""
    @State private var saving = false
    @State private var refusal: String?

    init(current: MetadataSettingsRecord, onSave: @escaping (Bool, MetadataProvider, String, String) async -> String?) {
        self.current = current
        self.onSave = onSave
        _enabled = State(initialValue: current.enabled)
        _provider = State(initialValue: current.provider)
        _language = State(initialValue: current.language)
    }

    var body: some View {
        DialogSheet("Metadata enrichment") {
            Text("Off by default: enabling sends the titles in your library to a third party, associated with your IP address and subject to their privacy policy. With AniList, the matched ids also go to ani.zip for episode names and wide artwork.")
            Toggle("Enable enrichment", isOn: $enabled).tint(scheme.primary).foregroundStyle(scheme.onSurface).padding(.vertical, 6)
            HStack(spacing: 8) {
                ForEach([MetadataProvider.aniList, .tmdb], id: \.self) { option in
                    Button(option.displayName) { provider = option }
                        .buttonStyle(option == provider ? ProtonButtonStyle(kind: .accent) : ProtonButtonStyle(kind: .edged))
                }
            }
            Text(provider == .aniList ? "Anime; no account or API key required." : "Film and television; requires a free TMDB API key.")
                .textStyle(.bodySmall)
            if provider == .tmdb {
                Field(label: "Language", text: $language)
                Field(label: current.ready ? "TMDB API key (leave blank to keep)" : "TMDB API key", text: $apiKey, secret: true)
            }
            if enabled, provider == .tmdb, !current.ready, apiKey.trimmingCharacters(in: .whitespaces).isEmpty {
                Text("Enter a TMDB API key to save.").foregroundStyle(scheme.error)
            }
            if let refusal {
                Text(refusal).foregroundStyle(scheme.error)
            }
        } buttons: {
            Button("Cancel") { dismiss() }.buttonStyle(.tonal)
            Button(saving ? "Saving…" : "Save") {
                saving = true
                refusal = nil
                Task {
                    refusal = await onSave(enabled, provider, language.trimmingCharacters(in: .whitespaces).isEmpty ? "en" : language, apiKey)
                    saving = false
                    if refusal == nil { dismiss() }
                }
            }
            .buttonStyle(.accent)
            .disabled(saving || !(!enabled || provider != .tmdb || current.ready || !apiKey.trimmingCharacters(in: .whitespaces).isEmpty))
        }
        .interactiveDismissDisabled(saving)
        .secureContent()
    }
}

private struct LegalDocument: Identifiable {
    let title: String
    let resource: String
    let type: String
    var id: String { resource }
}

private struct LegalDocumentSheet: View {
    let document: LegalDocument
    @Environment(\.dismiss) private var dismiss
    @State private var contents = "Loading…"

    var body: some View {
        DialogSheet(document.title) {
            Text(contents).textStyle(.bodySmall).textSelection(.enabled)
        } buttons: {
            Button("Close") { dismiss() }.buttonStyle(.accent)
        }
        .presentationDetents([.large])
        .task {
            let url = Bundle.main.url(forResource: document.resource, withExtension: document.type)
            do {
                guard let url else { throw CocoaError(.fileNoSuchFile) }
                contents = try String(contentsOf: url, encoding: .utf8)
            } catch {
                contents = "Unable to load this document: \(error.localizedDescription)"
            }
        }
    }
}

/// Flavour, accent and gradients — the same three the desktop client offers.
///
/// Every colour is resolved by Rust, so a swatch here is the colour the app
/// will actually paint rather than an approximation of it, and the choice is
/// stored in the file both clients read.
private struct AppearancePicker: View {
    let onNamesChanged: () -> Void
    @Environment(AppModel.self) private var model
    @Environment(\.scheme) private var scheme
    @State private var choice: AppearanceRecord?
    @State private var swatches: [AccentChoice: Color] = [:]

    private static let accents: [AccentChoice] = [.mauve, .pink, .sky, .pinkSky, .lavender, .blue, .teal, .peach, .red]
    private static let flavors: [FlavorChoice] = [.proton, .latte, .frappe, .macchiato, .mocha, .persona5]
    private static let names: [TitleNamesChoice] = [.library, .english, .romaji]

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let current = choice {
                Text("Palette").textStyle(.bodyMedium).foregroundStyle(scheme.onSurface).padding(.top, 8)
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        ForEach(Self.flavors, id: \.self) { flavor in
                            Button(flavor.label) { change { $0.flavor = flavor } }
                                .buttonStyle(flavor == current.flavor ? ProtonButtonStyle(kind: .accent) : ProtonButtonStyle(kind: .edged))
                        }
                    }
                    .padding(.vertical, 8)
                }
                Text("Accent").textStyle(.bodyMedium).foregroundStyle(scheme.onSurface)
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 10) {
                        ForEach(Self.accents, id: \.self) { accent in
                            let selected = accent == current.accent
                            Circle()
                                .fill(swatches[accent] ?? scheme.surfaceVariant)
                                .frame(width: 36, height: 36)
                                .overlay(Circle().strokeBorder(selected ? scheme.onBackground : scheme.outline, lineWidth: selected ? 3 : 1))
                                .onTapGesture { change { $0.accent = accent } }
                                .accessibilityLabel(String(describing: accent))
                                .accessibilityAddTraits(selected ? [.isButton, .isSelected] : .isButton)
                        }
                    }
                    .padding(.vertical, 8)
                }
                Text("Title names").textStyle(.bodyMedium).foregroundStyle(scheme.onSurface)
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        ForEach(Self.names, id: \.self) { names in
                            Button(names.label) { change(namesChanged: true) { $0.names = names } }
                                .buttonStyle(names == current.names ? ProtonButtonStyle(kind: .accent) : ProtonButtonStyle(kind: .edged))
                        }
                    }
                    .padding(.vertical, 8)
                }
                Text("What a matched title is called. Its other names still find it in search.")
                    .textStyle(.bodySmall).foregroundStyle(scheme.onSurfaceVariant).padding(.bottom, 8)
                Toggle("Paint the accent as a gradient", isOn: Binding(
                    get: { current.gradients },
                    set: { on in change { $0.gradients = on } }
                ))
                .tint(scheme.primary)
                .foregroundStyle(scheme.onSurface)
                .padding(.vertical, 14)
                Text("Off is the safer setting on a panel that bands: a slow ramp across a wide bar shows every step it is drawn from, and flat is better than striped.")
                    .textStyle(.bodySmall).foregroundStyle(scheme.onSurfaceVariant).padding(.bottom, 14)
            }
        }
        .task {
            if let stored = try? await model.run({ try $0.appearance() }) {
                await repaint(stored, store: false)
            }
        }
    }

    /// Selected at once, so a second tap builds on the first rather than on
    /// what was there before it, and painted when Rust has resolved it.
    private func change(namesChanged: Bool = false, _ edit: (inout AppearanceRecord) -> Void) {
        guard var next = choice else { return }
        edit(&next)
        choice = next
        let chosen = next
        Task {
            await repaint(chosen, store: true)
            if namesChanged { onNamesChanged() }
        }
    }

    private func repaint(_ next: AppearanceRecord, store: Bool) async {
        let accents = Self.accents
        let resolved: (PaletteRecord, [AccentChoice: UInt32])
        do {
            resolved = try await model.run { engine -> (PaletteRecord, [AccentChoice: UInt32]) in
                if store { try engine.setAppearance(appearance: next) }
                let palette = engine.previewPalette(appearance: next)
                // Every accent as it would look in *this* flavour: a swatch row that
                // keeps Mocha's pastels while Latte is selected lies about what the
                // next tap does.
                var row: [AccentChoice: UInt32] = [:]
                for accent in accents {
                    var variant = next
                    variant.accent = accent
                    row[accent] = engine.previewPalette(appearance: variant).accent
                }
                return (palette, row)
            }
        } catch {
            model.reportError(error)
            return
        }
        // A later tap has already moved on; its own repaint is on the way.
        if store, choice != next { return }
        let (palette, row) = resolved
        choice = next
        swatches = row.mapValues(argb)
        AppearanceState.shared.apply(palette)
    }
}

private extension TitleNamesChoice {
    /// What each name setting is called. The desktop client says the same.
    var label: String {
        switch self {
        case .library: "As in the share"
        case .english: "English"
        case .romaji: "Romaji"
        }
    }
}

private extension FlavorChoice {
    /// What each palette family is called. The desktop client says the same.
    var label: String {
        switch self {
        case .proton: "Proton"
        case .latte: "Catppuccin Latte"
        case .frappe: "Catppuccin Frappé"
        case .macchiato: "Catppuccin Macchiato"
        case .mocha: "Catppuccin Mocha"
        case .persona5: "Persona 5"
        }
    }
}
