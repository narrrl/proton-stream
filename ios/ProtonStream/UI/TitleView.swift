import SwiftUI

/// How wide the page's text and controls run on a window wider than a phone.
private let readableWidth: CGFloat = 720

struct TitleView: View {
    let title: TitleRecord
    let onPlay: ([EpisodeRecord], Int) -> Void
    let onBack: () -> Void
    /// Every title of its franchise, in release order, this one included.
    var franchise: [TitleRecord] = []
    var onOpenTitle: (TitleRecord) -> Void = { _ in }

    @Environment(AppModel.self) private var model
    @Environment(\.scheme) private var scheme
    @Environment(\.openURL) private var openURL
    @State private var showMatch = false
    @State private var overviewOpen = false
    @State private var seasonIndex: Int
    // Back by an edge swipe: the page shrinks towards the library as the
    // gesture is dragged, and springs back if it is let go of — Android's
    // predictive back, so the viewer sees where back goes before committing.
    @State private var backProgress: CGFloat = 0

    init(
        title: TitleRecord,
        onPlay: @escaping ([EpisodeRecord], Int) -> Void,
        onBack: @escaping () -> Void,
        franchise: [TitleRecord] = [],
        onOpenTitle: @escaping (TitleRecord) -> Void = { _ in }
    ) {
        self.title = title
        self.onPlay = onPlay
        self.onBack = onBack
        self.franchise = franchise
        self.onOpenTitle = onOpenTitle
        // One season on screen at a time, opened on the one Play would land in.
        let upNext = title.playlist.indices.contains(nextUpIndex(title.playlist)) ? title.playlist[nextUpIndex(title.playlist)] : nil
        _seasonIndex = State(initialValue: max(title.seasons.firstIndex { $0.episodes.contains { $0.linkId == upNext?.linkId } } ?? 0, 0))
    }

    var body: some View {
        // Display order across seasons: what previous/next and autoplay walk.
        let playlist = title.playlist
        let nextUp = nextUpIndex(playlist)
        let upNext = playlist.indices.contains(nextUp) ? playlist[nextUp] : nil
        let season = title.seasons.indices.contains(seasonIndex) ? title.seasons[seasonIndex] : nil
        let downloads = DownloadCoordinator.shared.records

        GeometryReader { geometry in
            // On a tablet the page keeps a phone's reading width, centred. The
            // backdrop still spans the window, but no taller than about half.
            let side = max((geometry.size.width - readableWidth) / 2, 0)
            let backdropHeight = min(geometry.size.width * 9 / 16, geometry.size.height * 0.55)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 8) {
                    backdrop(height: backdropHeight + geometry.safeAreaInsets.top, top: geometry.safeAreaInsets.top)
                    heading(playlist: playlist, nextUp: nextUp, upNext: upNext)
                        .padding(.horizontal, side + 16)
                    if franchise.count > 1 {
                        franchiseRow(side: side)
                    }
                    seasonChips(season: season, side: side)
                    if let season {
                        ForEach(Array(season.episodes.enumerated()), id: \.element.linkId) { position, episode in
                            EpisodeRow(
                                episode: episode,
                                numbering: episode.numbering(season.number, position),
                                position: position,
                                download: downloads.first { $0.shareId == episode.shareId && $0.linkId == episode.linkId },
                                onPlay: { onPlay(playlist, playlist.firstIndex { $0.linkId == episode.linkId } ?? 0) },
                                onSetWatched: { model.setWatched(episode, $0) }
                            )
                            .padding(.horizontal, side)
                        }
                    }
                }
                .padding(.bottom, 24)
            }
            .ignoresSafeArea(edges: .top)
            .background(scheme.background)
        }
        .scaleEffect(1 - 0.1 * backProgress)
        .clipShape(RoundedRectangle(cornerRadius: 32 * backProgress))
        .overlay(alignment: .leading) {
            Color.clear
                .frame(width: 20)
                .contentShape(Rectangle())
                .gesture(
                    DragGesture(minimumDistance: 10)
                        .onChanged { drag in backProgress = min(max(drag.translation.width / 300, 0), 1) }
                        .onEnded { drag in
                            if drag.translation.width > 100 || drag.predictedEndTranslation.width > 250 {
                                onBack()
                            }
                            withAnimation(.spring) { backProgress = 0 }
                        }
                )
        }
        .sheet(isPresented: $showMatch) {
            ChangeMatchSheet(title: title) { showMatch = false }
        }
        .onChange(of: title.key) {
            overviewOpen = false
            let playlist = title.playlist
            let upNext = playlist.indices.contains(nextUpIndex(playlist)) ? playlist[nextUpIndex(playlist)] : nil
            seasonIndex = title.seasons.firstIndex { $0.episodes.contains { $0.linkId == upNext?.linkId } } ?? 0
        }
    }

    /// Edge to edge, fading into the page: the art is the first thing the page
    /// says.
    private func backdrop(height: CGFloat, top: CGFloat) -> some View {
        RemoteArtwork(title.backdropUrl ?? title.posterUrl, title.displayName, fallback: title.thumbnailSource, labelled: false)
            .frame(height: height)
            .frame(maxWidth: .infinity)
            .clipped()
            .overlay {
                LinearGradient(
                    stops: [.init(color: .clear, location: 0.45), .init(color: scheme.background, location: 1)],
                    startPoint: .top,
                    endPoint: .bottom
                )
            }
            .overlay(alignment: .topLeading) {
                Button(action: onBack) {
                    Image(systemName: "chevron.backward")
                        .font(.system(size: 18, weight: .semibold))
                        .foregroundStyle(.white)
                        .frame(width: 40, height: 40)
                        .background(.black.opacity(0.4), in: Circle())
                        .frame(width: 48, height: 48)
                }
                .accessibilityLabel("Back to library")
                .padding(8)
                .padding(.top, top)
            }
    }

    @ViewBuilder private func heading(playlist: [EpisodeRecord], nextUp: Int, upNext: EpisodeRecord?) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(title.displayName).textStyle(.headlineMedium).foregroundStyle(scheme.onSurface)
            // The other names it goes by: the share's when the shown one is the
            // provider's, and the original-language one.
            let names: [String?] = [
                title.name != title.displayName ? title.name : nil,
                title.originalName.flatMap { $0 != title.displayName && $0 != title.canonicalName ? $0 : nil },
            ]
            let aliases = names.compactMap { $0 }.joined(separator: "  ·  ")
            if !aliases.isEmpty {
                Text(aliases).textStyle(.bodyMedium).foregroundStyle(scheme.onSurfaceVariant)
            }
            Text(factLine)
                .textStyle(.bodySmall).foregroundStyle(scheme.onSurfaceVariant).padding(.top, 6)
            // Who made it, and whether more is coming.
            let makers: [String?] = [
                title.studios.isEmpty ? nil : title.studios.prefix(2).joined(separator: ", "),
                title.directors.first.map { "dir. \($0)" },
                airingNote(title),
            ]
            let facts = makers.compactMap { $0 }.joined(separator: "  ·  ")
            if !facts.isEmpty {
                Text(facts).textStyle(.bodySmall).foregroundStyle(scheme.onSurfaceVariant).padding(.top, 2)
            }
            // The one thing the page is for, full width and naming what it will
            // play: "Resume" alone left the viewer guessing which episode.
            Button { onPlay(playlist, nextUp) } label: {
                Label(playLabel(playlist: playlist, upNext: upNext), systemImage: "play.fill")
            }
            .buttonStyle(.accentWide)
            .disabled(playlist.isEmpty)
            .padding(.top, 16)
            HStack(spacing: 0) {
                // Only where it would do something different: an unstarted show
                // already starts at the beginning.
                if upNext?.resumeAt != nil || title.watchedCount > 0 {
                    TitleAction(icon: "arrow.counterclockwise", label: "Start over") { onPlay(playlist, 0) }
                }
                TitleAction(icon: "arrow.down.circle", label: "Download") { DownloadCoordinator.shared.enqueue(playlist) }
                TitleAction(icon: "pencil", label: "Match") { showMatch = true }
                // The provider's own page for this title: where a viewer goes to
                // check that the thing the app matched is what they have.
                if let link = title.externalUrl.flatMap(URL.init(string:)) {
                    TitleAction(icon: "arrow.up.right.square", label: title.metadataProvider?.displayName ?? "Provider") {
                        openURL(link)
                    }
                }
            }
            .padding(.top, 12)
            if let overview = title.overview {
                // Three lines, then a tap for the rest.
                Text(overview.trimmingCharacters(in: .whitespacesAndNewlines))
                    .textStyle(.bodyLarge)
                    .foregroundStyle(scheme.onSurface)
                    .lineLimit(overviewOpen ? nil : 3)
                    .padding(.top, 12)
                    .onTapGesture { overviewOpen.toggle() }
            }
            if !title.genres.isEmpty || !title.tags.isEmpty {
                Text((title.genres + title.tags.prefix(4)).joined(separator: "  ·  "))
                    .textStyle(.labelMedium).foregroundStyle(scheme.onSurfaceVariant).padding(.top, 10)
            }
            // Said plainly, because it changes what a re-match will do.
            if title.manualMatch {
                Text("Matched by hand").textStyle(.labelMedium).foregroundStyle(scheme.onSurfaceVariant).padding(.top, 8)
            }
        }
    }

    private var factLine: String {
        let year: String? = (title.metadataYear ?? title.year).map { String($0) }
        let rating: String? = title.rating.map { String(format: "★ %.1f", $0) }
        let watched: String? = title.episodeCount > 1 ? "\(title.watchedCount) of \(title.episodeCount) watched" : nil
        let parts: [String?] = [title.formatLabel, title.seasonLabel ?? year, rating, watched]
        return parts.compactMap { $0 }.joined(separator: "  ·  ")
    }

    private func playLabel(playlist: [EpisodeRecord], upNext: EpisodeRecord?) -> String {
        guard let upNext else { return "Play" }
        if upNext.resumeAt != nil { return "Resume \(upNext.label)" }
        if title.watchedCount > 0 { return "Continue with \(upNext.label)" }
        if playlist.count == 1 { return "Play" }
        return "Play \(upNext.label)"
    }

    private func franchiseRow(side: CGFloat) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("In this franchise").textStyle(.titleMedium).foregroundStyle(scheme.onSurface)
                .padding(.horizontal, side + 16)
            ScrollView(.horizontal, showsIndicators: false) {
                LazyHStack(alignment: .top, spacing: 12) {
                    ForEach(franchise, id: \.key) { member in
                        FranchiseTile(title: member, here: member.key == title.key)
                            .onTapGesture { if member.key != title.key { onOpenTitle(member) } }
                    }
                }
                .padding(.horizontal, side + 16)
            }
        }
        .padding(.top, 12)
    }

    private func seasonChips(season: SeasonRecord?, side: CGFloat) -> some View {
        HStack(spacing: 0) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    ForEach(Array(title.seasons.enumerated()), id: \.element.label) { index, entry in
                        FilterChip(label: entry.label, selected: index == seasonIndex) { seasonIndex = index }
                    }
                }
            }
            if let season {
                IconButton("arrow.down.circle", "Download \(season.label)") {
                    DownloadCoordinator.shared.enqueue(season.episodes)
                }
            }
        }
        .padding(.leading, side + 16)
        .padding(.trailing, side + 4)
        .padding(.top, 12)
    }
}

/// Material's filter chip: outlined, or filled with a tick when selected.
struct FilterChip: View {
    let label: String
    let selected: Bool
    let action: () -> Void
    @Environment(\.scheme) private var scheme

    var body: some View {
        Button(action: action) {
            HStack(spacing: 6) {
                if selected { Image(systemName: "checkmark").font(.system(size: 13, weight: .semibold)) }
                Text(label).textStyle(.labelLarge)
            }
            .padding(.horizontal, 12)
            .frame(height: 32)
            .foregroundStyle(selected ? scheme.onSecondaryContainer : scheme.onSurfaceVariant)
            .background(selected ? scheme.secondaryContainer : .clear, in: RoundedRectangle(cornerRadius: Corner.standard))
            .overlay {
                if !selected { RoundedRectangle(cornerRadius: Corner.standard).strokeBorder(scheme.outlineVariant) }
            }
        }
        .buttonStyle(.plain)
    }
}

/// One part of the franchise: its cover, what it is and when. This title's own
/// is marked rather than left out, so the row says where it sits in the story.
private struct FranchiseTile: View {
    let title: TitleRecord
    let here: Bool
    @Environment(\.scheme) private var scheme

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            RemoteArtwork(title.posterUrl ?? title.backdropUrl, title.displayName, fallback: title.thumbnailSource)
                .aspectRatio(2 / 3, contentMode: .fit)
                .clipShape(RoundedRectangle(cornerRadius: Corner.standard))
                .overlay {
                    if here { RoundedRectangle(cornerRadius: Corner.standard).strokeBorder(scheme.primary, lineWidth: 2) }
                }
            Text(title.displayName).textStyle(.labelMedium, semibold: here).foregroundStyle(scheme.onSurface)
                .lineLimit(2).padding(.top, 6)
            Text([title.formatLabel ?? (title.kind == .film ? "Film" : "Series"), (title.metadataYear ?? title.year).map(String.init)]
                .compactMap { $0 }.joined(separator: " · "))
                .textStyle(.labelSmall).foregroundStyle(scheme.onSurfaceVariant).lineLimit(1)
        }
        .frame(width: 96)
        .contentShape(Rectangle())
    }
}

/// "episode 7 on Sat 11 Oct" while one is due, "airing" while it is airing
/// with no date yet, else nothing.
private func airingNote(_ title: TitleRecord) -> String? {
    if let at = title.nextAiringAt, let episode = title.nextEpisode {
        let format = DateFormatter()
        format.setLocalizedDateFormatFromTemplate("EEE d MMM")
        return "episode \(episode) on " + format.string(from: Date(timeIntervalSince1970: TimeInterval(at)))
    }
    return title.airing ? "airing" : nil
}

/// One of the title's secondary actions: an icon with its name under it.
private struct TitleAction: View {
    let icon: String
    let label: String
    let action: () -> Void
    @Environment(\.scheme) private var scheme

    var body: some View {
        Button(action: action) {
            VStack(spacing: 4) {
                Image(systemName: icon).font(.system(size: 22))
                Text(label).textStyle(.labelMedium).lineLimit(1)
            }
            .foregroundStyle(scheme.onSurface)
            .padding(.vertical, 8)
            .frame(maxWidth: .infinity)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

extension MetadataProvider {
    var displayName: String {
        switch self {
        case .aniList: "AniList"
        case .tmdb: "TMDB"
        }
    }
}

extension EpisodeRecord {
    /// `S03E38`, `E38`, or the row's place in the season.
    ///
    /// Mirrors `Episode::numbering` in `pstr-core`, plus the desktop client's
    /// fallback to the season folder's number. The last case is deliberately
    /// *not* printed as an episode number — `#4` says "fourth in the list"
    /// where `E04` would be a claim about the show.
    func numbering(_ fallbackSeason: UInt32?, _ position: Int) -> String {
        switch (season ?? fallbackSeason, number) {
        case let (season?, number?): String(format: "S%02dE%02d", season, number)
        case let (nil, number?): String(format: "E%02d", number)
        default: "#\(position + 1)"
        }
    }
}

/// One episode: play it, see where it was left, and drive its download.
///
/// The row *is* the play button — the still, the number and the name are one
/// target, which is what lets the trailing controls be two icons in a fixed
/// column rather than a second row of buttons under every episode.
private struct EpisodeRow: View {
    let episode: EpisodeRecord
    let numbering: String
    let position: Int
    let download: RetainedDownload?
    let onPlay: () -> Void
    let onSetWatched: (Bool) -> Void
    @Environment(\.scheme) private var scheme

    private var subline: String {
        let size: String? = episode.size.map { formatBytes($0) }
        let parts: [String?] = [numbering, size, episode.airDate]
        return parts.compactMap { $0 }.joined(separator: " · ")
    }

    var body: some View {
        let running = download?.status == RetainedDownload.statusRunning || download?.status == RetainedDownload.statusQueued
        let heading = episode.heading(position)
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 0) {
                RemoteArtwork(episode.stillUrl, heading, fallback: episode.thumbnailSource, labelled: false)
                    .frame(width: 128)
                    .aspectRatio(16 / 9, contentMode: .fit)
                    .overlay(alignment: .bottom) {
                        if let progress = episode.progress, !episode.watched { AccentProgress(progress: progress) }
                    }
                    .clipShape(RoundedRectangle(cornerRadius: Corner.standard))
                VStack(alignment: .leading, spacing: 2) {
                    // Seen episodes stay legible but stop competing with the
                    // one the viewer has not watched yet.
                    Text(heading).textStyle(.titleSmall)
                        .foregroundStyle(episode.watched ? scheme.onSurfaceVariant : scheme.onSurface)
                        .lineLimit(2)
                    Text(subline)
                        .textStyle(.bodySmall).foregroundStyle(scheme.onSurfaceVariant).lineLimit(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 12)
                // Watched is a judgement the viewer is allowed to overrule.
                IconButton(
                    episode.watched ? "checkmark.circle.fill" : "checkmark.circle",
                    episode.watched ? "Mark unwatched" : "Mark watched",
                    tint: episode.watched ? scheme.primary : scheme.outline
                ) { onSetWatched(!episode.watched) }
                // One slot, always the same width, whatever state the download
                // is in.
                Group {
                    if episode.offline {
                        Image(systemName: "checkmark.icloud")
                            .font(.system(size: 20))
                            .foregroundStyle(scheme.tertiary)
                            .accessibilityLabel("Saved offline")
                    } else if running, let download {
                        IconButton("pause.fill", "Pause download") { DownloadCoordinator.shared.pause(download) }
                    } else if let download {
                        IconButton("arrow.down.circle", "Resume download") { DownloadCoordinator.shared.resume(download) }
                    } else {
                        IconButton("arrow.down.circle", "Download") { DownloadCoordinator.shared.enqueue(episode) }
                    }
                }
                .frame(width: 48, height: 48)
            }
            if let overview = episode.providerOverview {
                Text(overview).textStyle(.bodySmall).foregroundStyle(scheme.onSurfaceVariant).lineLimit(2).padding(.top, 6)
            }
            if let active = download, active.total > 0, !episode.offline {
                AccentProgress(progress: Double(active.downloaded) / Double(active.total), solid: scheme.tertiary)
                    .padding(.top, 8)
                Text(downloadStatusLine(active)).textStyle(.bodySmall).foregroundStyle(scheme.onSurfaceVariant)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 6)
        .contentShape(Rectangle())
        .onTapGesture(perform: onPlay)
    }
}

/// Search the configured provider for the right entry, choose it, or forget
/// the match altogether.
struct ChangeMatchSheet: View {
    let title: TitleRecord
    let onDone: () -> Void
    @Environment(AppModel.self) private var model
    @Environment(\.scheme) private var scheme
    @State private var term: String
    @State private var searching = false
    // Choosing fetches the entry's episodes too, which is a few requests.
    @State private var choosing = false
    @State private var options: [MatchRecord] = []
    /// Said here rather than in the snackbar, which this sheet covers.
    @State private var failure: String?
    @State private var searched = false

    init(title: TitleRecord, onDone: @escaping () -> Void) {
        self.title = title
        self.onDone = onDone
        _term = State(initialValue: title.canonicalName ?? title.name)
    }

    var body: some View {
        DialogSheet("Change match") {
            Text("Search the configured metadata provider. Nothing is stored until you choose an entry.")
            Field(label: "Title", text: $term, submit: .search)
                .onSubmit(search)
            Button(searching ? "Searching…" : "Search", action: search)
                .buttonStyle(.accent)
                .disabled(term.trimmingCharacters(in: .whitespaces).isEmpty || searching)
            if choosing {
                HStack(spacing: 12) {
                    ProgressView().controlSize(.small).tint(scheme.primary)
                    Text("Fetching its episodes…").textStyle(.bodySmall)
                }
                .padding(.top, 8)
            }
            if let failure {
                Text(failure).foregroundStyle(scheme.error)
            }
            if !searching, options.isEmpty, searched {
                Text("Nothing found. Try the title's original or English name.").textStyle(.bodySmall)
            }
            ForEach(options, id: \.remoteId) { option in
                Button { choose(option) } label: {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(option.name).textStyle(.labelLarge, semibold: true)
                        Text([option.year.map(String.init), option.originalName].compactMap { $0 }.joined(separator: " · "))
                            .textStyle(.labelLarge)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.vertical, 8)
                }
                .buttonStyle(.edgedWide)
                .disabled(choosing)
            }
        } buttons: {
            Button("Forget match") {
                Task {
                    do {
                        try await model.run { try $0.forgetMatch(titleKey: title.key) }
                        changed()
                    } catch {
                        failure = errorMessage(error)
                    }
                }
            }
            .buttonStyle(.quiet)
            Button("Close", action: onDone).buttonStyle(.tonal)
        }
    }

    private func search() {
        let term = term
        guard !term.trimmingCharacters(in: .whitespaces).isEmpty, !searching else { return }
        searching = true
        failure = nil
        Task {
            do {
                options = try await NativeRuntime.engine().searchMatches(titleKey: title.key, term: term)
            } catch {
                failure = errorMessage(error)
            }
            searching = false
            searched = true
        }
    }

    private func choose(_ option: MatchRecord) {
        choosing = true
        failure = nil
        Task {
            do {
                try await NativeRuntime.engine().chooseMatch(titleKey: title.key, found: option)
                changed()
            } catch {
                failure = errorMessage(error)
            }
            choosing = false
        }
    }

    private func changed() {
        onDone()
        model.reloadAfterMetadataChange()
    }
}
