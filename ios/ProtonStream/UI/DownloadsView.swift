import SwiftUI

/// One show's saved episodes, and the show itself where the library has it.
private struct OfflineGroup: Identifiable {
    let name: String
    let title: TitleRecord?
    let files: [OfflineRecord]
    var id: String { name }
}

struct DownloadsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scheme) private var scheme
    // Deleting a saved episode asks first: it is the viewer's own choice to
    // keep it, and getting it back is a download, not a tap.
    @State private var deleting: OfflineRecord?

    var body: some View {
        let retained = DownloadCoordinator.shared.records
        let offline = model.offline
        TabPage("Downloads") {
            if retained.isEmpty && offline.isEmpty {
                EmptyState("No downloads", "Episodes, seasons and shows saved offline appear here.")
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        if !offline.isEmpty {
                            Text("\(offline.count) \(offline.count == 1 ? "episode" : "episodes") · \(formatBytes(offline.reduce(0) { $0 + $1.size })) on this device")
                                .textStyle(.bodyMedium)
                                .foregroundStyle(scheme.onSurfaceVariant)
                                .padding(.horizontal, 16)
                                .padding(.vertical, 4)
                        }
                        // What is still arriving comes first: it is the part of
                        // the page that changes, and the part with something to do.
                        if !retained.isEmpty {
                            SectionHeading(title: "In progress")
                            ForEach(retained) { download in
                                PartialRow(download: download)
                            }
                        }
                        // Under the show they belong to, as on desktop: a saved
                        // season is fourteen rows, and fourteen rows of
                        // "Episode 3" name nothing.
                        ForEach(groups(offline)) { group in
                            SectionHeading(
                                title: group.name,
                                caption: "\(group.files.count) \(group.files.count == 1 ? "episode" : "episodes") · \(formatBytes(group.files.reduce(0) { $0 + $1.size }))"
                            )
                            ForEach(group.files, id: \.linkId) { file in
                                SavedRow(file: file, title: group.title) { deleting = file }
                            }
                        }
                    }
                    .padding(.bottom, 24)
                }
            }
        }
        .alert(
            "Delete this download?",
            isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }),
            presenting: deleting
        ) { file in
            Button("Delete", role: .destructive) { model.removeOffline(file) }
            Button("Cancel", role: .cancel) {}
        } message: { file in
            Text("\(file.episode?.label ?? "The episode") (\(formatBytes(file.size))) leaves this device. It still streams, and can be downloaded again.")
        }
    }

    /// Which show each saved file is from. The offline record knows its
    /// episode but not its show, and the library is the only thing that does.
    private func groups(_ offline: [OfflineRecord]) -> [OfflineGroup] {
        var shows: [String: TitleRecord] = [:]
        for title in model.titles {
            for episode in title.playlist { shows["\(episode.shareId)/\(episode.linkId)"] = title }
        }
        let grouped = Dictionary(grouping: offline) { shows["\($0.shareId)/\($0.linkId)"]?.key }
        return grouped.map { key, files in
            let title = key.flatMap { key in model.titles.first { $0.key == key } }
            return OfflineGroup(name: title?.displayName ?? "Not in the library", title: title, files: files)
        }
        .sorted { $0.name < $1.name }
    }
}

private struct SectionHeading: View {
    let title: String
    var caption: String?
    @Environment(\.scheme) private var scheme

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(title).textStyle(.titleMedium).foregroundStyle(scheme.onSurface).lineLimit(1)
            if let caption { Text(caption).textStyle(.bodySmall).foregroundStyle(scheme.onSurfaceVariant) }
        }
        .padding(EdgeInsets(top: 20, leading: 16, bottom: 4, trailing: 16))
    }
}

/// A saved episode: its still, what it is, its size, and a way to remove it.
private struct SavedRow: View {
    let file: OfflineRecord
    let title: TitleRecord?
    let onDelete: () -> Void
    @Environment(\.scheme) private var scheme

    var body: some View {
        let episode = file.episode
        HStack(spacing: 16) {
            RemoteArtwork(episode?.stillUrl ?? title?.backdropUrl, title?.name ?? file.linkId, fallback: episode?.thumbnailSource, labelled: episode == nil)
                .frame(width: 96)
                .aspectRatio(16 / 9, contentMode: .fit)
                .clipShape(RoundedRectangle(cornerRadius: Corner.standard))
            VStack(alignment: .leading, spacing: 2) {
                Text(episode?.heading(0) ?? file.linkId).textStyle(.bodyLarge).foregroundStyle(scheme.onSurface).lineLimit(2)
                Text([episode?.label, formatBytes(file.size)].compactMap { $0 }.joined(separator: " · "))
                    .textStyle(.bodyMedium).foregroundStyle(scheme.onSurfaceVariant)
            }
            Spacer(minLength: 0)
            IconButton("trash", "Delete download", action: onDelete)
        }
        .padding(.leading, 16)
        .padding(.trailing, 4)
        .padding(.vertical, 8)
    }
}

/// A download not yet finished: how far it has got, why it stopped if it did,
/// and pause or resume beside cancel.
private struct PartialRow: View {
    let download: RetainedDownload
    @Environment(AppModel.self) private var model
    @Environment(\.scheme) private var scheme

    var body: some View {
        let active = download.status == RetainedDownload.statusRunning || download.status == RetainedDownload.statusQueued
        HStack(spacing: 8) {
            VStack(alignment: .leading, spacing: 2) {
                Text(download.label).textStyle(.bodyLarge).foregroundStyle(scheme.onSurface).lineLimit(2)
                Text(downloadStatusLine(download)).textStyle(.bodyMedium).foregroundStyle(scheme.onSurfaceVariant)
                if download.total > 0 {
                    AccentProgress(progress: Double(download.downloaded) / Double(download.total)).padding(.top, 8)
                }
                if let error = download.error {
                    Text(error).textStyle(.bodyMedium).foregroundStyle(scheme.error)
                }
            }
            Spacer(minLength: 0)
            if active {
                IconButton("pause.fill", "Pause download") { model.pauseDownload(download) }
            } else {
                IconButton("play.fill", "Resume download") { model.resumeDownload(download) }
            }
            // What has arrived so far is discarded; nothing finished is.
            IconButton("xmark", "Cancel and delete what has arrived") { model.deletePartial(download) }
        }
        .padding(.leading, 16)
        .padding(.trailing, 4)
        .padding(.vertical, 8)
    }
}
