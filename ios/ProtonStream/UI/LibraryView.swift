import SwiftUI
import UIKit

/// How many shows the Continue watching shelf holds: a shelf is a shortcut,
/// not a second library.
private let continueWatchingMax = 12

/// One row of the Continue watching shelf: the episode, and where it sits.
struct Resumable: Identifiable {
    let title: TitleRecord
    let episode: EpisodeRecord
    let index: Int

    var id: String { "\(episode.shareId)/\(episode.linkId)" }
}

/// One tile of the grid: the title drawn, and how many more of its franchise
/// it stands for.
private struct Tile: Identifiable {
    let title: TitleRecord
    let folded: Int
    var id: String { title.key }
}

/// A row above the grid: titles that share a director, a studio or a genre.
private struct Shelf: Identifiable {
    let label: String
    let titles: [TitleRecord]
    var id: String { label }
}

/// Most recently played first, one episode per show: a shelf that lists four
/// episodes of the same series is a shelf with room for nothing else.
func resumable(_ titles: [TitleRecord]) -> [Resumable] {
    titles.compactMap { title -> Resumable? in
        let playlist = title.playlist
        guard let index = playlist.indices
            .filter({ playlist[$0].resumeAt != nil && !playlist[$0].watched })
            .max(by: { playlist[$0].lastPlayed < playlist[$1].lastPlayed })
        else { return nil }
        return Resumable(title: title, episode: playlist[index], index: index)
    }
    .sorted { $0.episode.lastPlayed > $1.episode.lastPlayed }
    .prefix(continueWatchingMax)
    .map { $0 }
}

struct LibraryView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scheme) private var scheme
    let onResume: (TitleRecord, Int) -> Void
    let onTitle: (TitleRecord) -> Void

    // What a long press opened a menu for: a title, or a Continue watching card.
    @State private var menuFor: TitleRecord?
    @State private var menuResume: Resumable?
    @State private var matching: TitleRecord?
    // Search is an icon until it is wanted: a permanent field above the
    // library spent a band of every visit on something used in few of them.
    @SceneStorage("library.searching") private var searching = false
    @State private var collapsed = false

    var body: some View {
        let resumable = resumable(model.titles)
        VStack(spacing: 0) {
            if searching {
                SearchBar(query: Binding(get: { model.query }, set: model.search)) {
                    searching = false
                    model.search("")
                }
            } else {
                HStack(spacing: 0) {
                    // The large title folds into the bar as the grid scrolls.
                    Text("Library")
                        .textStyle(.titleLarge)
                        .foregroundStyle(scheme.onSurface)
                        .padding(.leading, 16)
                        .opacity(collapsed ? 1 : 0)
                    Spacer()
                    IconButton("magnifyingglass", "Search library") { searching = true }
                    ArrangeMenu(sort: model.sort, grouped: model.grouped, onArrange: model.arrange)
                    // Pull to refresh is the gesture; the icon stays for a
                    // viewer who does not know it.
                    IconButton("arrow.clockwise", "Refresh", enabled: !model.refreshing) { model.refresh() }
                }
                .frame(height: 56)
                .padding(.trailing, 4)
                .background(collapsed ? scheme.surfaceContainer : scheme.background)
            }
            content(resumable)
        }
        .background(scheme.background)
        .sheet(item: $menuFor) { title in
            let playlist = title.playlist
            let allWatched = title.episodeCount > 0 && title.watchedCount == title.episodeCount
            TileMenu(heading: title.displayName, caption: title.caption) {
                MenuRow("play.fill", playlist.contains { $0.resumeAt != nil } ? "Resume" : "Play") {
                    onResume(title, nextUpIndex(playlist))
                }
                MenuRow("info.circle", "Open") { onTitle(title) }
                MenuRow("arrow.down.circle", "Download all") { DownloadCoordinator.shared.enqueue(playlist) }
                MenuRow(allWatched ? "checkmark.circle" : "checkmark.circle.fill", allWatched ? "Mark unwatched" : "Mark watched") {
                    model.setTitleWatched(title, !allWatched)
                }
                // The same search the title page offers, for the tile that is
                // plainly wrong — the wrong poster is what the grid shows.
                MenuRow("pencil", "Change match") {
                    Task { @MainActor in
                        try? await Task.sleep(for: .milliseconds(350))
                        matching = title
                    }
                }
            }
        }
        .sheet(item: $menuResume) { entry in
            TileMenu(heading: entry.title.displayName, caption: entry.episode.label) {
                MenuRow("play.fill", "Resume") { onResume(entry.title, entry.index) }
                MenuRow("info.circle", "Open") { onTitle(entry.title) }
                MenuRow("minus.circle", "Remove from Continue watching") {
                    model.forgetPosition(entry.title, entry.episode)
                }
            }
        }
        .sheet(item: $matching) { title in
            ChangeMatchSheet(title: title) { matching = nil }
        }
    }

    @ViewBuilder private func content(_ resumable: [Resumable]) -> some View {
        if model.loading {
            LibrarySkeleton()
        } else if model.titles.isEmpty && !model.query.trimmingCharacters(in: .whitespaces).isEmpty {
            EmptyState("Nothing matches", "No title in the library is called “\(model.query.trimmingCharacters(in: .whitespaces))”.")
        } else if model.titles.isEmpty {
            ScrollView {
                EmptyState("Your library is empty", "Add a Proton Drive public link under Shares, then refresh.")
                    .containerRelativeFrame(.vertical)
            }
            .refreshable { await model.crawlNow(nil) }
        } else {
            grid(resumable)
        }
    }

    private func grid(_ resumable: [Resumable]) -> some View {
        // The arrangement names titles by key; before it has arrived, the
        // titles as they came are the grid.
        let byKey = Dictionary(model.titles.map { ($0.key, $0) }, uniquingKeysWith: { first, _ in first })
        let tiles = model.arrangement?.tiles.compactMap { tile in byKey[tile.key].map { Tile(title: $0, folded: Int(tile.folded)) } }
            ?? model.titles.map { Tile(title: $0, folded: 0) }
        let shelves = (model.arrangement?.shelves ?? [])
            .map { Shelf(label: $0.label, titles: $0.keys.compactMap { byKey[$0] }) }
            .filter { !$0.titles.isEmpty }
        let browsing = model.query.trimmingCharacters(in: .whitespaces).isEmpty
        let featured = browsing ? featuredTitle(model.titles, resumable) : nil

        return ScrollView {
            LazyVStack(alignment: .leading, spacing: 16) {
                if !searching {
                    Text("Library")
                        .textStyle(.headlineMedium)
                        .foregroundStyle(scheme.onSurface)
                        .background {
                            GeometryReader { geometry in
                                Color.clear.onChange(of: geometry.frame(in: .named("library")).maxY < 0) { _, folded in
                                    collapsed = folded
                                }
                            }
                        }
                }
                if let featured {
                    FeaturedBanner(
                        title: featured,
                        resume: resumable.first { $0.title.key == featured.key },
                        onPlay: { onResume(featured, $0) },
                        onOpen: { onTitle(featured) }
                    )
                }
                if browsing && !resumable.isEmpty {
                    VStack(alignment: .leading, spacing: 10) {
                        Text("Continue watching").textStyle(.titleMedium).foregroundStyle(scheme.onSurface)
                        ScrollView(.horizontal, showsIndicators: false) {
                            LazyHStack(alignment: .top, spacing: 12) {
                                ForEach(resumable) { entry in
                                    ContinueCard(entry: entry)
                                        .onTapGesture { onResume(entry.title, entry.index) }
                                        .onLongPressGesture { longPressed { menuResume = entry } }
                                }
                            }
                        }
                    }
                }
                if browsing {
                    ForEach(shelves) { shelf in
                        VStack(alignment: .leading, spacing: 10) {
                            Text(shelf.label).textStyle(.titleMedium).foregroundStyle(scheme.onSurface)
                            ScrollView(.horizontal, showsIndicators: false) {
                                LazyHStack(alignment: .top, spacing: 12) {
                                    ForEach(shelf.titles, id: \.key) { title in
                                        PosterTile(title: title)
                                            .frame(width: 112)
                                            .onTapGesture { onTitle(title) }
                                            .onLongPressGesture { longPressed { menuFor = title } }
                                    }
                                }
                            }
                        }
                    }
                    if !resumable.isEmpty || !shelves.isEmpty {
                        Text("All titles").textStyle(.titleMedium).foregroundStyle(scheme.onSurface).padding(.top, 8)
                    }
                }
                // Posters, three across on a phone.
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 108), spacing: 12, alignment: .top)], spacing: 16) {
                    ForEach(tiles) { tile in
                        PosterTile(title: tile.title, folded: tile.folded)
                            .onTapGesture { onTitle(tile.title) }
                            .onLongPressGesture { longPressed { menuFor = tile.title } }
                    }
                }
            }
            .padding(EdgeInsets(top: 8, leading: 16, bottom: 24, trailing: 16))
        }
        .coordinateSpace(name: "library")
        .refreshable { await model.crawlNow(nil) }
        .scrollDismissesKeyboard(.immediately)
    }

    private func longPressed(_ open: () -> Void) {
        UIImpactFeedbackGenerator(style: .medium).impactOccurred()
        open()
    }
}

/// The grid's order, and whether a franchise is one tile or one per title.
private struct ArrangeMenu: View {
    let sort: LibrarySort
    let grouped: Bool
    let onArrange: (LibrarySort, Bool) -> Void
    @Environment(\.scheme) private var scheme

    var body: some View {
        Menu {
            ForEach([LibrarySort.name, .recent, .added, .release, .rating, .popularity], id: \.self) { option in
                Button {
                    onArrange(option, grouped)
                } label: {
                    if option == sort { Label(option.label, systemImage: "checkmark") } else { Text(option.label) }
                }
            }
            Divider()
            Button {
                onArrange(sort, !grouped)
            } label: {
                if grouped { Label("One tile per franchise", systemImage: "checkmark") } else { Text("One tile per franchise") }
            }
        } label: {
            Image(systemName: "arrow.up.arrow.down")
                .font(.system(size: 20))
                .frame(width: 48, height: 48)
                .foregroundStyle(scheme.onSurfaceVariant)
        }
        .accessibilityLabel("Sort library")
    }
}

extension LibrarySort {
    /// What each order is called. The desktop says the same.
    var label: String {
        switch self {
        case .name: "A – Z"
        case .recent: "Recently watched"
        case .added: "Recently added"
        case .release: "Newest"
        case .rating: "Highest rated"
        case .popularity: "Most popular"
        }
    }
}

/// The bar while searching: back closes the search and clears it, and the
/// field takes the keyboard as soon as it opens.
private struct SearchBar: View {
    @Binding var query: String
    let onClose: () -> Void
    @FocusState private var focused: Bool
    @Environment(\.scheme) private var scheme

    var body: some View {
        HStack(spacing: 0) {
            IconButton("chevron.backward", "Close search", tint: scheme.onSurface, action: onClose)
            HStack {
                TextField("", text: $query, prompt: Text("Search library").foregroundStyle(scheme.onSurfaceVariant))
                    .textStyle(.bodyLarge)
                    .foregroundStyle(scheme.onSurface)
                    .focused($focused)
                    .submitLabel(.search)
                    .autocorrectionDisabled()
                if !query.isEmpty {
                    Button { query = "" } label: {
                        Image(systemName: "xmark").foregroundStyle(scheme.onSurfaceVariant)
                    }
                    .accessibilityLabel("Clear search")
                }
            }
            .padding(.horizontal, 16)
            .frame(height: 44)
            .background(scheme.surfaceContainerHighest, in: Capsule())
            .padding(.trailing, 12)
        }
        .frame(height: 56)
        .background(scheme.background)
        .onAppear { focused = true }
    }
}

/// A long press's menu, from the bottom of the screen where the thumb already
/// is. Each row closes it before doing its thing.
struct TileMenu<Rows: View>: View {
    let heading: String
    let caption: String
    @ViewBuilder var rows: Rows
    @Environment(\.scheme) private var scheme

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(heading).textStyle(.titleMedium).foregroundStyle(scheme.onSurface).lineLimit(1).padding(.horizontal, 24)
            Text(caption).textStyle(.bodySmall).foregroundStyle(scheme.onSurfaceVariant)
                .padding(.horizontal, 24).padding(.bottom, 8)
            rows
            Spacer(minLength: 0)
        }
        .padding(.top, 28)
        .presentationDetents([.height(CGFloat(110 + 56 * 5)), .large])
        .presentationDragIndicator(.visible)
        .presentationBackground(scheme.surfaceContainerLow)
        .presentationCornerRadius(Corner.sheet)
    }
}

struct MenuRow: View {
    let icon: String
    let label: String
    let action: () -> Void
    @Environment(\.dismiss) private var dismiss

    init(_ icon: String, _ label: String, action: @escaping () -> Void) {
        self.icon = icon
        self.label = label
        self.action = action
    }

    var body: some View {
        Button {
            dismiss()
            action()
        } label: {
            ListRow(icon: icon, headline: label)
        }
        .buttonStyle(.plain)
        .padding(.horizontal, 8)
    }
}

/// The grid's shape before the catalog has answered: posters and their two
/// lines of text as blocks, so the page does not jump when the titles land.
private struct LibrarySkeleton: View {
    var body: some View {
        ScrollView {
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 108), spacing: 12)], spacing: 16) {
                ForEach(0 ..< 18, id: \.self) { _ in
                    VStack(alignment: .leading, spacing: 0) {
                        Skeleton().aspectRatio(2 / 3, contentMode: .fit).clipShape(RoundedRectangle(cornerRadius: Corner.standard))
                        Skeleton().frame(height: 12).clipShape(RoundedRectangle(cornerRadius: Corner.standard))
                            .padding(.trailing, 20).padding(.top, 8)
                        Skeleton().frame(height: 10).clipShape(RoundedRectangle(cornerRadius: Corner.standard))
                            .padding(.trailing, 50).padding(.top, 6)
                    }
                }
            }
            .padding(EdgeInsets(top: 8, leading: 16, bottom: 24, trailing: 16))
        }
        .scrollDisabled(true)
    }
}

/// The title the banner shows: the one watched last, or with nothing part
/// watched, one of the titles the provider has a backdrop for, turning over
/// once a day — the desktop's `featured`.
private func featuredTitle(_ titles: [TitleRecord], _ resumable: [Resumable]) -> TitleRecord? {
    if let first = resumable.first { return first.title }
    let pictured = titles.filter { $0.backdropUrl != nil }
    if pictured.isEmpty { return nil }
    let day = Int(Date().timeIntervalSince1970 + Double(TimeZone.current.secondsFromGMT())) / 86400
    return pictured[day % pictured.count]
}

/// Large art behind the name, rating, genres and three lines of synopsis.
private struct FeaturedBanner: View {
    let title: TitleRecord
    let resume: Resumable?
    let onPlay: (Int) -> Void
    let onOpen: () -> Void

    var body: some View {
        RemoteArtwork(title.backdropUrl ?? title.posterUrl, title.displayName, fallback: title.thumbnailSource, labelled: false)
            .aspectRatio(4 / 3, contentMode: .fit)
            .overlay {
                LinearGradient(
                    stops: [.init(color: .clear, location: 0.35), .init(color: .black.opacity(0.85), location: 1)],
                    startPoint: .top,
                    endPoint: .bottom
                )
            }
            .overlay(alignment: .bottomLeading) {
                VStack(alignment: .leading, spacing: 0) {
                    Text(title.displayName).textStyle(.headlineMedium).foregroundStyle(.white).lineLimit(2)
                    Text(factLine).textStyle(.bodySmall).foregroundStyle(.white.opacity(0.8))
                    if let overview = title.overview {
                        Text(overview.trimmingCharacters(in: .whitespacesAndNewlines))
                            .textStyle(.bodySmall).foregroundStyle(.white.opacity(0.8)).lineLimit(3).padding(.top, 6)
                    }
                    HStack(spacing: 8) {
                        Button { onPlay(resume?.index ?? nextUpIndex(title.playlist)) } label: {
                            Label(resume != nil ? "Resume" : "Play", systemImage: "play.fill")
                        }
                        .buttonStyle(.accent)
                        Button("More info", action: onOpen).buttonStyle(.tonal)
                    }
                    .padding(.top, 12)
                }
                .padding(16)
            }
            .clipShape(RoundedRectangle(cornerRadius: Corner.standard))
    }

    private var factLine: String {
        let parts = [
            title.rating.map { String(format: "★ %.1f", $0) },
            title.genres.isEmpty ? nil : title.genres.prefix(3).joined(separator: ", "),
        ].compactMap { $0 }
        return parts.isEmpty ? title.caption : parts.joined(separator: "  ·  ")
    }
}

/// A part-watched episode: its still, where it was left, and what is left of it.
private struct ContinueCard: View {
    let entry: Resumable
    @Environment(\.scheme) private var scheme

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            RemoteArtwork(entry.episode.stillUrl ?? entry.title.backdropUrl, entry.title.displayName, fallback: entry.episode.thumbnailSource)
                .aspectRatio(16 / 9, contentMode: .fit)
                .overlay(alignment: .bottom) {
                    if let progress = entry.episode.progress { AccentProgress(progress: progress) }
                }
                .clipShape(RoundedRectangle(cornerRadius: Corner.standard))
            Text(entry.title.displayName).textStyle(.bodyLarge, semibold: true).foregroundStyle(scheme.onSurface)
                .lineLimit(1).padding(.top, 8)
            Text([entry.episode.label, entry.episode.timeLeft].compactMap { $0 }.joined(separator: " · "))
                .textStyle(.bodySmall).foregroundStyle(scheme.onSurfaceVariant).lineLimit(1)
        }
        .frame(width: 220)
        .contentShape(Rectangle())
    }
}

/// One title: its poster, and its name and size under it rather than in a card.
///
/// `folded` is how many more titles of its franchise the tile stands for, said
/// in the corner.
struct PosterTile: View {
    let title: TitleRecord
    var folded = 0
    @Environment(\.scheme) private var scheme

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            RemoteArtwork(title.posterUrl ?? title.backdropUrl, title.displayName, fallback: title.thumbnailSource)
                .aspectRatio(2 / 3, contentMode: .fit)
                .clipShape(RoundedRectangle(cornerRadius: Corner.standard))
                .overlay(alignment: .topTrailing) {
                    if folded > 0 {
                        Text("+\(folded)")
                            .textStyle(.labelSmall)
                            .foregroundStyle(.white)
                            .padding(.horizontal, 6).padding(.vertical, 2)
                            .background(.black.opacity(0.7), in: RoundedRectangle(cornerRadius: Corner.standard))
                            .padding(6)
                    }
                }
            Text(title.displayName).textStyle(.bodyMedium, semibold: true).foregroundStyle(scheme.onSurface)
                .lineLimit(2).padding(.top, 6)
            // Two lines, not one: at a large text size "2023 · 7 episodes"
            // does not fit a poster's width.
            Text(title.caption).textStyle(.bodySmall).foregroundStyle(scheme.onSurfaceVariant).lineLimit(2)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
    }
}

extension TitleRecord {
    /// "2023 · 11 episodes", or "1988 · Film" — never "1 episodes".
    var caption: String {
        let count: String
        if (metadataKind ?? kind) == .film && episodeCount == 1 {
            count = "Film"
        } else if episodeCount == 1 {
            count = "1 episode"
        } else {
            count = "\(episodeCount) episodes"
        }
        return [(metadataYear ?? year).map(String.init), count].compactMap { $0 }.joined(separator: " · ")
    }
}

extension EpisodeRecord {
    /// "12 min left", from where the episode was left and how far through that
    /// is. Nil when either is unknown — a guess would be worse than no figure.
    var timeLeft: String? {
        guard let at = resumeAt, let fraction = progress, fraction > 0, fraction < 1 else { return nil }
        let minutes = Int((at / fraction - at) / 60)
        return minutes < 1 ? "under a minute left" : "\(minutes) min left"
    }
}

extension TitleRecord: Identifiable {
    public var id: String { key }
}
