import SwiftUI

struct Played: Identifiable {
    let title: TitleRecord
    let episode: EpisodeRecord
    let index: Int

    var id: String { "\(episode.shareId)/\(episode.linkId)" }
}

/// Every episode started or finished, newest first — the desktop's
/// `Library::history`, over the records the library already holds.
///
/// A row marked unwatched with its position at zero is "never played" for the
/// purpose here, and left out. Progress stands in for the position: it is
/// non-zero exactly when the position is.
func history(_ titles: [TitleRecord]) -> [Played] {
    titles.flatMap { title in
        title.playlist.enumerated()
            .filter { $0.element.watched || ($0.element.progress ?? 0) > 0 }
            .map { Played(title: title, episode: $0.element, index: $0.offset) }
    }
    .sorted { $0.episode.lastPlayed > $1.episode.lastPlayed }
}

/// "Today", "Yesterday", a weekday within the last week, then a date — how the
/// history page heads each day.
func dayHeading(_ day: Date, today: Date, calendar: Calendar = .current, locale: Locale = .current) -> String {
    let day = calendar.startOfDay(for: day)
    let today = calendar.startOfDay(for: today)
    let days = calendar.dateComponents([.day], from: day, to: today).day ?? 0
    if days == 0 { return "Today" }
    if days == 1 { return "Yesterday" }
    let format = DateFormatter()
    format.locale = locale
    format.calendar = calendar
    format.timeZone = calendar.timeZone
    if days > 0, days < 7 {
        format.dateFormat = "EEEE"
    } else if calendar.component(.year, from: day) == calendar.component(.year, from: today) {
        format.dateFormat = "d MMMM"
    } else {
        format.dateFormat = "d MMMM yyyy"
    }
    return format.string(from: day)
}

private struct PlayedDay {
    let day: Date
    var rows: [Played]
}

struct HistoryView: View {
    let onPlay: (TitleRecord, Int) -> Void
    let onTitle: (TitleRecord) -> Void
    @Environment(AppModel.self) private var model
    @Environment(\.scheme) private var scheme

    var body: some View {
        let calendar = Calendar.current
        let played = history(model.titles)
        // Grouped in order: the list is newest first, so each day's rows are
        // already together.
        let days = played.reduce(into: [PlayedDay]()) { days, entry in
            let day = calendar.startOfDay(for: Date(timeIntervalSince1970: TimeInterval(entry.episode.lastPlayed)))
            if days.last?.day == day { days[days.count - 1].rows.append(entry) } else { days.append(PlayedDay(day: day, rows: [entry])) }
        }
        TabPage("History") {
            if days.isEmpty {
                EmptyState("Nothing watched yet", "Episodes you play show up here, newest first.")
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(days, id: \.day) { day in
                            Text(dayHeading(day.day, today: Date()))
                                .textStyle(.titleSmall)
                                .foregroundStyle(scheme.primary)
                                .padding(EdgeInsets(top: 20, leading: 16, bottom: 4, trailing: 16))
                            ForEach(day.rows) { entry in
                                HistoryRow(
                                    entry: entry,
                                    onPlay: { onPlay(entry.title, entry.index) },
                                    onTitle: { onTitle(entry.title) },
                                    onRemove: { model.removeFromHistory(entry.title, entry.episode) }
                                )
                            }
                        }
                    }
                    .padding(.bottom, 24)
                }
            }
        }
    }
}

/// The still plays the episode, the text opens its title, and the cross takes
/// it off the page — the same three targets the desktop row has.
private struct HistoryRow: View {
    let entry: Played
    let onPlay: () -> Void
    let onTitle: () -> Void
    let onRemove: () -> Void
    @Environment(\.scheme) private var scheme

    var body: some View {
        let episode = entry.episode
        HStack(spacing: 0) {
            RemoteArtwork(episode.stillUrl ?? entry.title.backdropUrl, episode.label, fallback: episode.thumbnailSource, labelled: false)
                .frame(width: 128)
                .aspectRatio(16 / 9, contentMode: .fit)
                .overlay(alignment: .bottom) {
                    if let progress = episode.progress, !episode.watched { AccentProgress(progress: progress) }
                }
                .clipShape(RoundedRectangle(cornerRadius: Corner.standard))
                .onTapGesture(perform: onPlay)
            VStack(alignment: .leading, spacing: 0) {
                Text(entry.title.displayName).textStyle(.titleSmall).foregroundStyle(scheme.onSurface).lineLimit(1)
                Text(episode.providerName.map { "\(episode.label) · \($0)" } ?? episode.label)
                    .textStyle(.bodySmall).foregroundStyle(scheme.onSurface).lineLimit(1)
                Text(episode.watched ? "Finished" : episode.stoppedAt)
                    .textStyle(.bodySmall).foregroundStyle(scheme.onSurfaceVariant)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 4)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
            .onTapGesture(perform: onTitle)
            IconButton("xmark", "Remove from history", action: onRemove)
        }
        .padding(EdgeInsets(top: 6, leading: 16, bottom: 6, trailing: 4))
    }
}

private extension EpisodeRecord {
    /// "Stopped at 12:04 of 23:40", or as much of it as is known.
    var stoppedAt: String {
        guard let at = resumeAt else { return "Started" }
        let duration = progress.flatMap { $0 > 0 ? at / $0 : nil }
        return "Stopped at \(clock(at))" + (duration.map { " of \(clock($0))" } ?? "")
    }
}
