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
    /// When the title was last played, which orders the shelf.
    let lastPlayed: Int64

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
/// episodes of the same series is a shelf with room for nothing else. Which
/// episode is the engine's call (`continueIndex`): the part-watched one, or the
/// one after the episode finished last.
func resumable(_ titles: [TitleRecord]) -> [Resumable] {
    titles.compactMap { title -> Resumable? in
        guard let index = title.continueIndex.map(Int.init) else { return nil }
        let playlist = title.playlist
        guard playlist.indices.contains(index) else { return nil }
        return Resumable(
            title: title,
            episode: playlist[index],
            index: index,
            lastPlayed: playlist.map(\.lastPlayed).max() ?? 0
        )
    }
    .sorted { $0.lastPlayed > $1.lastPlayed }
    .prefix(continueWatchingMax)
    .map { $0 }
}

struct LibraryView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scheme) private var scheme
    let onResume: (TitleRecord, Int) -> Void
    let onTitle: (TitleRecord) -> Void
    var onAddShare: () -> Void = {}
    var onOpenSettings: () -> Void = {}
    /// Open every title sharing a genre, studio or director a search named.
    var onBrowse: (BrowseFacet) -> Void = { _ in }

    // What a long press opened a menu for: a title, or a Continue watching card.
    @State private var menuFor: TitleRecord?
    @State private var menuResume: Resumable?
    @State private var matching: TitleRecord?
    @State private var downloading: DownloadRequest?
    // Posters and synopses, offered once there is a library to put them on
    // and never turned on unasked: switching them on sends title names out.
    @State private var offerDismissed = SettingsStore().metadataOfferDismissed
    /// The same, for a download asked about once the menu has gone.
    @State private var pendingDownload: DownloadRequest?
    /// "Change match" picked in the tile menu, waiting for that menu to finish
    /// leaving: a sheet presented while another is still dismissing is dropped.
    @State private var pendingMatch: TitleRecord?
    // Search is an icon until it is wanted: a permanent field above the
    // library spent a band of every visit on something used in few of them.
    @SceneStorage("library.searching") private var searching = false
    @State private var collapsed = false
    /// How far the bar over the banner has filled in, from clear to solid.
    @State private var reveal: CGFloat = 0

    /// The page's bar: its title, shown as much as `solid` says, and its
    /// actions, in `ink` or the scheme's own.
    private func bar(solid: CGFloat, ink: Color?) -> some View {
        HStack(spacing: 0) {
            Text("Library")
                .textStyle(.titleLarge)
                .foregroundStyle(scheme.onSurface)
                .padding(.leading, 16)
                .opacity(solid)
            Spacer()
            IconButton("magnifyingglass", "Search library", tint: ink) { searching = true }
            ArrangeMenu(sort: model.sort, grouped: model.grouped, onArrange: model.arrange)
            // Pull to refresh is the gesture; the icon stays for a viewer who
            // does not know it.
            IconButton("arrow.clockwise", "Refresh", tint: ink, enabled: !model.refreshing) { model.refresh() }
        }
        .frame(height: 56)
        .padding(.trailing, 4)
    }

    var body: some View {
        let resumable = resumable(model.titles)
        let browsing = model.query.trimmingCharacters(in: .whitespaces).isEmpty
        let featured = featuredTitles(model.titles, resumable)
        // With a banner, the bar lies over it, clear, and fills in as the
        // banner scrolls away; without one, the large title folds into the bar
        // as the grid scrolls.
        let overBanner = !searching && !model.loading && browsing && !featured.isEmpty
        ZStack(alignment: .top) {
            VStack(spacing: 0) {
                if searching {
                    SearchBar(query: Binding(get: { model.query }, set: model.search)) {
                        searching = false
                        model.search("")
                    }
                } else if !overBanner {
                    bar(solid: collapsed ? 1 : 0, ink: nil)
                        .background(collapsed ? scheme.surfaceContainer : scheme.background)
                }
                content(resumable, featured: overBanner ? featured : [])
            }
            if overBanner {
                // White over the banner's own scrim, then the page's ink once
                // the bar has filled in.
                bar(solid: reveal, ink: reveal > 0.5 ? nil : .white)
                    .background(scheme.surfaceContainer.opacity(reveal).ignoresSafeArea(edges: .top))
                    .animation(.easeOut(duration: 0.15), value: reveal > 0.5)
            }
        }
        .background(scheme.background)
        .sheet(item: $menuFor, onDismiss: {
            // After the dismissal rather than a guessed delay: on a slow iPad
            // the menu was still leaving after 350 ms, the search sheet was
            // dropped, and it turned up later on some unrelated redraw.
            if let title = pendingMatch {
                pendingMatch = nil
                matching = title
            }
            downloading = pendingDownload
            pendingDownload = nil
        }) { title in
            let playlist = title.playlist
            let allWatched = title.episodeCount > 0 && title.watchedCount == title.episodeCount
            TileMenu(heading: title.displayName, caption: title.caption) {
                MenuRow("play.fill", playlist.contains { $0.resumeAt != nil } ? "Resume" : "Play") {
                    onResume(title, nextUpIndex(playlist, continueIndex: title.continueIndex))
                }
                MenuRow("info.circle", "Open") { onTitle(title) }
                MenuRow(title.onList ? "checkmark" : "plus", title.onList ? "Remove from My List" : "Add to My List") {
                    model.setOnList(title, !title.onList)
                }
                MenuRow("arrow.down.circle", "Download all") { pendingDownload = download(playlist, title.displayName) }
                MenuRow(allWatched ? "checkmark.circle" : "checkmark.circle.fill", allWatched ? "Mark unwatched" : "Mark watched") {
                    model.setTitleWatched(title, !allWatched)
                }
                // The same search the title page offers, for the tile that is
                // plainly wrong — the wrong poster is what the grid shows.
                MenuRow("pencil", "Change match") { pendingMatch = title }
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
        .confirmingDownload($downloading)
        .sheet(item: $matching) { title in
            ChangeMatchSheet(title: title) { matching = nil }
        }
    }

    @ViewBuilder private func content(_ resumable: [Resumable], featured: [TitleRecord]) -> some View {
        if model.loading {
            LibrarySkeleton()
        } else if searching && model.query.trimmingCharacters(in: .whitespaces).isEmpty && !model.recentSearches.isEmpty {
            // Empty and with the field open: the last few searches.
            recentSearches
        } else if !model.query.trimmingCharacters(in: .whitespaces).isEmpty,
                  model.arrangement?.tiles.isEmpty == true,
                  model.searchExtras?.isEmpty ?? true {
            EmptyState("Nothing matches", "Nothing in the library is called “\(model.query.trimmingCharacters(in: .whitespaces))”.")
        } else if model.titles.isEmpty, let activity = model.activity {
            // A share just added: say it is being looked through, rather than
            // offer to add the one that is already filling the page.
            EmptyState("Looking through the share", "\(activity) Its titles appear here once it has been listed.")
        } else if model.titles.isEmpty {
            ScrollView {
                EmptyState(
                    "Your library is empty",
                    "Add a Proton Drive share link, and everything playable behind it shows up here.",
                    action: ("Add a share", onAddShare)
                )
                    .containerRelativeFrame(.vertical)
            }
            .refreshable { await model.crawlNow(nil) }
        } else {
            grid(resumable, featured: featured)
        }
    }

    private func grid(_ resumable: [Resumable], featured: [TitleRecord]) -> some View {
        // The arrangement names titles by key; before it has arrived, the
        // titles as they came are the grid.
        let byKey = Dictionary(model.titles.map { ($0.key, $0) }, uniquingKeysWith: { first, _ in first })
        let tiles = model.arrangement?.tiles.compactMap { tile in byKey[tile.key].map { Tile(title: $0, folded: Int(tile.folded)) } }
            ?? model.titles.map { Tile(title: $0, folded: 0) }
        let shelves = (model.arrangement?.shelves ?? [])
            .map { Shelf(label: $0.label, titles: $0.keys.compactMap { byKey[$0] }) }
            .filter { !$0.titles.isEmpty }
        let browsing = model.query.trimmingCharacters(in: .whitespaces).isEmpty

        return ScrollView {
            LazyVStack(alignment: .leading, spacing: 16) {
                if !featured.isEmpty {
                    FeaturedBanner(titles: featured, resumable: resumable, onPlay: onResume, onOpen: onTitle)
                        // Edge to edge, out of the grid's margin.
                        .padding(.horizontal, -16)
                        .background {
                            GeometryReader { geometry in
                                Color.clear.onChange(of: geometry.frame(in: .named("library")).minY) { _, top in
                                    reveal = min(max(-top / 280, 0), 1)
                                }
                            }
                        }
                        // Scrolled far enough to leave the lazy stack: the bar
                        // is long since solid.
                        .onDisappear { reveal = 1 }
                        .onAppear { reveal = 0 }
                } else if !searching {
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
                if browsing, !model.metadataSettings.enabled, !offerDismissed {
                    MetadataOfferCard(onOpenSettings: onOpenSettings) {
                        offerDismissed = true
                        SettingsStore().metadataOfferDismissed = true
                    }
                }
                if browsing && !resumable.isEmpty {
                    VStack(alignment: .leading, spacing: 10) {
                        Text("Continue watching").textStyle(.titleMedium).foregroundStyle(scheme.onSurface)
                        ScrollView(.horizontal, showsIndicators: false) {
                            LazyHStack(alignment: .top, spacing: 12) {
                                ForEach(resumable) { entry in
                                    ContinueCard(entry: entry)
                                        .tapAndHold { onResume(entry.title, entry.index) } onHold: {
                                            longPressed { menuResume = entry }
                                        }
                                }
                            }
                        }
                    }
                }
                // Only over the whole library: under a filter, a shelf of
                // titles the grid below is hiding reads as the filter not
                // working.
                if browsing && model.filter == .all {
                    ForEach(shelves) { shelf in
                        VStack(alignment: .leading, spacing: 10) {
                            Text(shelf.label).textStyle(.titleMedium).foregroundStyle(scheme.onSurface)
                            ScrollView(.horizontal, showsIndicators: false) {
                                LazyHStack(alignment: .top, spacing: 12) {
                                    ForEach(shelf.titles, id: \.key) { title in
                                        // On a shelf, an airing show says when its
                                        // next episode is out.
                                        PosterTile(title: title, badge: title.badge ?? title.countdown())
                                            .frame(width: 112)
                                            .tapAndHold { onTitle(title) } onHold: { longPressed { menuFor = title } }
                                    }
                                }
                            }
                        }
                    }
                    if !resumable.isEmpty || !shelves.isEmpty {
                        Text("All titles").textStyle(.titleMedium).foregroundStyle(scheme.onSurface).padding(.top, 8)
                    }
                }
                // What the search found besides titles, above the titles it
                // found.
                if !browsing, let extras = model.searchExtras {
                    searchExtras(extras, byKey: byKey)
                    if !tiles.isEmpty && !extras.isEmpty {
                        Text("Titles").textStyle(.titleMedium).foregroundStyle(scheme.onSurface)
                    }
                }
                if browsing {
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 8) {
                            ForEach(SettingsStore.filters, id: \.self) { filter in
                                FilterChip(label: filter.label, selected: filter == model.filter) { model.setFilter(filter) }
                            }
                        }
                    }
                    if tiles.isEmpty && model.filter != .all {
                        Text("Nothing in the library fits that filter.")
                            .textStyle(.bodyMedium)
                            .foregroundStyle(scheme.onSurfaceVariant)
                    }
                }
                // Posters, three across on a phone.
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 108), spacing: 12, alignment: .top)], spacing: 16) {
                    ForEach(tiles) { tile in
                        PosterTile(title: tile.title, folded: tile.folded)
                            .tapAndHold { onTitle(tile.title) } onHold: { longPressed { menuFor = tile.title } }
                    }
                }
            }
            // No room above a banner: it starts under the status bar, and the
            // bar lies over it.
            .padding(EdgeInsets(top: featured.isEmpty ? 8 : 0, leading: 16, bottom: 24, trailing: 16))
        }
        .ignoresSafeArea(edges: featured.isEmpty ? [] : .top)
        .coordinateSpace(name: "library")
        .refreshable { await model.crawlNow(nil) }
        .scrollDismissesKeyboard(.immediately)
    }

    /// The people, studios and genres a search named as chips, then the
    /// episodes it found by name. Android's `facetChips` and its episode rows.
    @ViewBuilder private func searchExtras(_ extras: SearchExtrasRecord, byKey: [String: TitleRecord]) -> some View {
        ForEach([("People and studios", extras.people), ("Genres", extras.genres)], id: \.0) { heading, facets in
            if !facets.isEmpty {
                VStack(alignment: .leading, spacing: 8) {
                    Text(heading).textStyle(.titleMedium).foregroundStyle(scheme.onSurface)
                    FlowLayout(spacing: 6) {
                        ForEach(facets, id: \.facet.label) { found in
                            GenreChip(text: "\(found.facet.label) · \(found.count)") { onBrowse(found.facet) }
                        }
                    }
                }
            }
        }
        if !extras.episodes.isEmpty {
            VStack(alignment: .leading, spacing: 4) {
                Text("Episodes").textStyle(.titleMedium).foregroundStyle(scheme.onSurface)
                ForEach(extras.episodes, id: \.linkId) { hit in
                    if let title = byKey[hit.titleKey],
                       let index = title.playlist.firstIndex(where: { $0.linkId == hit.linkId }) {
                        Button { onResume(title, index) } label: {
                            HStack(spacing: 12) {
                                Image(systemName: "play.fill").foregroundStyle(scheme.onSurfaceVariant)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(hit.name).textStyle(.bodyLarge).foregroundStyle(scheme.onSurface).lineLimit(1)
                                    Text("\(title.displayName) · \(hit.label)")
                                        .textStyle(.bodySmall).foregroundStyle(scheme.onSurfaceVariant).lineLimit(1)
                                }
                                Spacer(minLength: 0)
                            }
                            .padding(.vertical, 8)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityHint("Play")
                    }
                }
            }
        }
    }

    /// The last few searches, under an empty search field.
    private var recentSearches: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                Text("Recent searches").textStyle(.titleMedium).foregroundStyle(scheme.onSurface).padding(.vertical, 8)
                ForEach(model.recentSearches, id: \.self) { query in
                    Button { model.search(query) } label: {
                        Label(query, systemImage: "clock.arrow.circlepath")
                            .textStyle(.bodyLarge)
                            .foregroundStyle(scheme.onSurface)
                            .frame(maxWidth: .infinity, minHeight: 48, alignment: .leading)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
                Button("Clear recent searches") { model.forgetSearches() }
                    .buttonStyle(.quiet)
                    .padding(.top, 8)
            }
            .padding(.horizontal, 16)
        }
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
        .buttonStyle(.pressable)
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

extension LibraryFilter {
    /// The desktop's names for the filters, from `browse::Filter::label`.
    var label: String {
        switch self {
        case .all: "All"
        case .series: "Series"
        case .films: "Films"
        case .watching: "Watching"
        case .unwatched: "Not started"
        case .downloaded: "Downloaded"
        case .myList: "My List"
        }
    }
}

/// The offer of posters and synopses, and its two answers.
private struct MetadataOfferCard: View {
    let onOpenSettings: () -> Void
    let onDismiss: () -> Void
    @Environment(\.scheme) private var scheme

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Add posters and synopses?").textStyle(.titleSmall).foregroundStyle(scheme.onSurface)
            Text("Turning it on in Settings sends the titles in your library — not your files, and nothing about what you have watched — to AniList or TMDB.")
                .textStyle(.bodySmall)
                .foregroundStyle(scheme.onSurfaceVariant)
            HStack {
                Spacer()
                Button("Not now", action: onDismiss).buttonStyle(.quiet)
                Button("Open settings", action: onOpenSettings).buttonStyle(.quiet)
            }
        }
        .padding(EdgeInsets(top: 12, leading: 16, bottom: 4, trailing: 8))
        .background(scheme.surfaceContainer, in: RoundedRectangle(cornerRadius: 12))
    }
}

/// How many titles the banner turns through: the desktop's `FEATURED`.
private let featuredMax = 5

/// How long each title holds the banner: the desktop's `HOLD`.
private let holdSeconds = 8.0

/// One title giving way to the next: the desktop's `TURN`.
private let turn = Animation.easeInOut(duration: 0.6)

/// The titles the banner turns through: the one watched last first, then
/// titles the provider has a backdrop for, starting from a different one each
/// day — the desktop's `featured`.
private func featuredTitles(_ titles: [TitleRecord], _ resumable: [Resumable]) -> [TitleRecord] {
    var picked = resumable.first.map { [$0.title] } ?? []
    let pictured = titles.filter { $0.backdropUrl != nil }
    if pictured.isEmpty { return picked }
    let day = Int(Date().timeIntervalSince1970 + Double(TimeZone.current.secondsFromGMT())) / 86400
    for offset in pictured.indices {
        if picked.count == featuredMax { break }
        let title = pictured[(day + offset) % pictured.count]
        if !picked.contains(where: { $0.key == title.key }) { picked.append(title) }
    }
    return picked
}

/// The banner at the top of the library: one title at a time, edge to edge
/// under the status bar, turning to the next every `holdSeconds` — a swipe
/// starts the count again. Each title's veil is tinted with its own picture's
/// colour.
private struct FeaturedBanner: View {
    let titles: [TitleRecord]
    let resumable: [Resumable]
    let onPlay: (TitleRecord, Int) -> Void
    let onOpen: (TitleRecord) -> Void
    @Environment(\.scheme) private var scheme
    @State private var shown = ""

    var body: some View {
        let current = titles.contains { $0.key == shown } ? shown : titles.first?.key ?? ""
        TabView(selection: Binding(get: { current }, set: { shown = $0 })) {
            ForEach(titles, id: \.key) { title in
                FeaturedPage(
                    title: title,
                    resume: resumable.first { $0.title.key == title.key },
                    onPlay: { onPlay(title, $0) },
                    onOpen: { onOpen(title) }
                )
                .tag(title.key)
            }
        }
        .tabViewStyle(.page(indexDisplayMode: .never))
        // Most of a phone's first screen, but never so much of an iPad's that
        // nothing under it shows.
        .containerRelativeFrame(.vertical) { height, _ in min(max(height * 0.62, 360), 620) }
        .overlay(alignment: .bottom) {
            if titles.count > 1 {
                HStack(spacing: 6) {
                    ForEach(titles, id: \.key) { title in
                        let selected = title.key == current
                        Capsule()
                            .fill(scheme.onBackground.opacity(selected ? 1 : 0.4))
                            .frame(width: selected ? 18 : 6, height: 6)
                            .contentShape(Rectangle().inset(by: -8))
                            .onTapGesture { withAnimation(turn) { shown = title.key } }
                            .accessibilityLabel("Show \(title.displayName)")
                            .accessibilityAddTraits(selected ? [.isButton, .isSelected] : .isButton)
                    }
                }
                .animation(.easeOut(duration: 0.15), value: current)
                .padding(.bottom, 10)
            }
        }
        .task(id: current) {
            guard titles.count > 1 else { return }
            try? await Task.sleep(for: .seconds(holdSeconds))
            if Task.isCancelled { return }
            let index = titles.firstIndex { $0.key == current } ?? 0
            withAnimation(turn) { shown = titles[(index + 1) % titles.count].key }
        }
    }
}

/// One title on the banner: its art, its veil, and what it is, from the
/// bottom up.
private struct FeaturedPage: View {
    let title: TitleRecord
    let resume: Resumable?
    let onPlay: (Int) -> Void
    let onOpen: () -> Void
    @Environment(\.scheme) private var scheme
    @Environment(AppModel.self) private var model
    @State private var mood: UInt32?

    var body: some View {
        let veil = scheme.veil(mood)
        ZStack(alignment: .bottomLeading) {
            Backdrop(title: title) { mood = $0 }
            LinearGradient(
                stops: [
                    // Dark under the status bar and the bar's white icons…
                    .init(color: .black.opacity(0.5), location: 0),
                    .init(color: .clear, location: 0.22),
                    // …and the picture's own colour under the text, ending in
                    // the page it hands over to.
                    .init(color: veil.opacity(0.35), location: 0.45),
                    .init(color: veil.opacity(0.92), location: 0.78),
                    .init(color: scheme.background, location: 1),
                ],
                startPoint: .top,
                endPoint: .bottom
            )
            VStack(alignment: .leading, spacing: 0) {
                // A logo is lettering made for dark art, white as often as
                // not; over a light palette's veil it is gone, so the light
                // palette keeps the name.
                if scheme.light {
                    name
                } else {
                    RemoteLogo(title.logoUrl, title.displayName, maxWidth: 260, maxHeight: 88) { name }
                        .padding(.bottom, 8)
                }
                Text(factLine).textStyle(.bodySmall).foregroundStyle(scheme.onBackground.opacity(0.8))
                if let overview = title.overview {
                    Text(overview.trimmingCharacters(in: .whitespacesAndNewlines))
                        .textStyle(.bodySmall).foregroundStyle(scheme.onBackground.opacity(0.8)).lineLimit(2).padding(.top, 6)
                }
                HStack(spacing: 8) {
                    Button { onPlay(resume?.index ?? nextUpIndex(title.playlist, continueIndex: title.continueIndex)) } label: {
                        Label(resume?.episode.resumeAt != nil ? "Resume" : "Play", systemImage: "play.fill")
                    }
                    .buttonStyle(.accent)
                    Button(action: onOpen) { Label("More info", systemImage: "info.circle") }.buttonStyle(.tonal)
                    // The icon alone: a third labelled button does not fit a
                    // phone's row.
                    Button { model.setOnList(title, !title.onList) } label: {
                        Image(systemName: title.onList ? "checkmark" : "plus")
                    }
                    .buttonStyle(.tonal)
                    .accessibilityLabel(title.onList ? "Remove from My List" : "Add to My List")
                }
                .padding(.top, 12)
                if let episode = resume?.episode, episode.resumeAt != nil, let progress = episode.progress {
                    HStack(spacing: 8) {
                        AccentProgress(progress: progress).frame(width: 120, height: 4)
                        if let left = episode.timeLeft {
                            Text(left).textStyle(.bodySmall).foregroundStyle(scheme.onBackground.opacity(0.8))
                        }
                    }
                    .padding(.top, 12)
                }
            }
            .frame(maxWidth: 560, alignment: .leading)
            .padding(EdgeInsets(top: 0, leading: 16, bottom: 28, trailing: 16))
        }
        .animation(Motion.arrive, value: mood)
        .clipped()
        .contentShape(Rectangle())
        .onTapGesture(perform: onOpen)
    }

    private var name: some View {
        Text(title.displayName).textStyle(.displayLarge).foregroundStyle(scheme.onBackground).lineLimit(2).padding(.bottom, 4)
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
            // Named by the text under it; read as well, the name came twice.
            RemoteArtwork(entry.episode.stillUrl ?? entry.title.backdropUrl, entry.title.displayName, fallback: entry.episode.thumbnailSource)
                .accessibilityHidden(true)
                .aspectRatio(16 / 9, contentMode: .fit)
                .overlay(alignment: .bottom) {
                    if let progress = entry.episode.progress { AccentProgress(progress: progress) }
                }
                .clipShape(RoundedRectangle(cornerRadius: Corner.standard))
            Text(entry.title.displayName).textStyle(.bodyLarge, semibold: true).foregroundStyle(scheme.onSurface)
                .lineLimit(1).padding(.top, 8)
            Text(entry.episode.resumeAt == nil
                 ? "Next: \(entry.episode.label)"
                 : [entry.episode.label, entry.episode.timeLeft].compactMap { $0 }.joined(separator: " · "))
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
    /// `New`, `3 new`, or an airing show's countdown, at the top left.
    var badge: String?
    @Environment(\.scheme) private var scheme

    init(title: TitleRecord, folded: Int = 0, badge: String? = nil) {
        self.title = title
        self.folded = folded
        self.badge = badge ?? title.badge
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            RemoteArtwork(title.posterUrl ?? title.backdropUrl, title.displayName, fallback: title.thumbnailSource)
                .accessibilityHidden(true)
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
                            .accessibilityLabel("and \(folded) more")
                    }
                }
                .overlay(alignment: .topLeading) {
                    if let badge {
                        Text(badge)
                            .textStyle(.labelSmall, semibold: true)
                            .foregroundStyle(scheme.onPrimary)
                            .padding(.horizontal, 6).padding(.vertical, 2)
                            .background(scheme.primary, in: RoundedRectangle(cornerRadius: Corner.standard))
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
    /// `Ep 8 · in 2 d` for a show whose next episode is still to air.
    func countdown(now: Int64 = Int64(Date().timeIntervalSince1970)) -> String? {
        guard let at = nextAiringAt, at > now, let episode = nextEpisode else { return nil }
        return airingCountdown(episode: episode, at: at, now: now)
    }

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

/// Every title sharing a genre, a studio or a director: what a chip on a title
/// page opens. Android's `BrowseScreen`, the desktop's `library::browse`.
struct BrowseView: View {
    let facet: BrowseFacet
    let titles: [TitleRecord]
    let onBack: () -> Void
    let onTitle: (TitleRecord) -> Void
    @Environment(\.scheme) private var scheme

    var body: some View {
        TabPage(facet.label, leading: {
            IconButton("chevron.backward", "Back", tint: scheme.onSurface, action: onBack)
        }) {
            if titles.isEmpty {
                // A recrawl or a new match took the last one away.
                EmptyState(facet.label, "Nothing in the library has this any more.")
            } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("\(facetKind(facet: facet)) · \(titles.count == 1 ? "1 title" : "\(titles.count) titles")")
                            .textStyle(.bodyMedium)
                            .foregroundStyle(scheme.onSurfaceVariant)
                        LazyVGrid(columns: [GridItem(.adaptive(minimum: 108), spacing: 12, alignment: .top)], spacing: 16) {
                            ForEach(titles, id: \.key) { title in
                                PosterTile(title: title)
                                    .tapAndHold { onTitle(title) }
                            }
                        }
                    }
                    .padding(EdgeInsets(top: 8, leading: 16, bottom: 24, trailing: 16))
                }
            }
        }
    }
}

extension BrowseFacet {
    /// The name it stands for: the chip's text and its page's heading.
    var label: String {
        switch self {
        case .genre(let name), .studio(let name), .person(let name): name
        }
    }
}

extension SearchExtrasRecord {
    /// Whether a search found nothing besides titles.
    var isEmpty: Bool { episodes.isEmpty && people.isEmpty && genres.isEmpty }
}
