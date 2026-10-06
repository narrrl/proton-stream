import SwiftUI
import UIKit

/// A tab's page: its own name in a bar, over whatever the page holds.
///
/// The bar draws under the status bar itself; the mini transport's room at the
/// foot of the page is the shell's `safeAreaInset`, so scrolling content clears
/// it without being told.
struct TabPage<Content: View, Leading: View, Actions: View>: View {
    let title: String
    @ViewBuilder var leading: Leading
    @ViewBuilder var actions: Actions
    @ViewBuilder var content: Content
    @Environment(\.scheme) private var scheme

    init(
        _ title: String,
        @ViewBuilder leading: () -> Leading = { EmptyView() },
        @ViewBuilder actions: () -> Actions = { EmptyView() },
        @ViewBuilder content: () -> Content
    ) {
        self.title = title
        self.leading = leading()
        self.actions = actions()
        self.content = content()
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 0) {
                leading
                Text(title)
                    .textStyle(.titleLarge)
                    .foregroundStyle(scheme.onSurface)
                    .lineLimit(1)
                    .padding(.leading, Leading.self == EmptyView.self ? 16 : 4)
                Spacer(minLength: 8)
                actions
            }
            .frame(height: 56)
            .padding(.trailing, 4)
            content.frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .background(scheme.background)
    }
}

/// A 48pt icon target: what Material's `IconButton` is.
struct IconButton: View {
    let systemName: String
    let label: String
    var tint: Color?
    var enabled = true
    let action: () -> Void
    @Environment(\.scheme) private var scheme

    init(_ systemName: String, _ label: String, tint: Color? = nil, enabled: Bool = true, action: @escaping () -> Void) {
        self.systemName = systemName
        self.label = label
        self.tint = tint
        self.enabled = enabled
        self.action = action
    }

    var body: some View {
        Button(action: action) {
            Image(systemName: systemName)
                .font(.system(size: 20))
                .frame(width: 48, height: 48)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .foregroundStyle(enabled ? tint ?? scheme.onSurfaceVariant : scheme.onSurface.opacity(0.38))
        .disabled(!enabled)
        .accessibilityLabel(label)
    }
}

struct EmptyState: View {
    let title: String
    let body_: String
    @Environment(\.scheme) private var scheme

    init(_ title: String, _ body: String) {
        self.title = title
        body_ = body
    }

    var body: some View {
        VStack(spacing: 2) {
            Text(title).textStyle(.titleMedium).foregroundStyle(scheme.onSurface)
            Text(body_).textStyle(.bodyMedium).foregroundStyle(scheme.onSurface)
        }
        .multilineTextAlignment(.center)
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

/// A Material card: the container fill at the shared radius.
struct CardBackground: ViewModifier {
    @Environment(\.scheme) private var scheme
    var fill: Color?

    func body(content: Content) -> some View {
        content.background(fill ?? scheme.surfaceContainerHighest, in: RoundedRectangle(cornerRadius: Corner.standard))
    }
}

extension View {
    func card(_ fill: Color? = nil) -> some View { modifier(CardBackground(fill: fill)) }
}

/// One row of a list: a leading icon, a headline, a supporting line and a
/// trailing control — Material's `ListItem`.
struct ListRow<Trailing: View>: View {
    var icon: String?
    var iconTint: Color?
    let headline: String
    var supporting: String?
    var enabled = true
    @ViewBuilder var trailing: Trailing
    @Environment(\.scheme) private var scheme

    var body: some View {
        HStack(spacing: 16) {
            if let icon {
                Image(systemName: icon)
                    .font(.system(size: 20))
                    .foregroundStyle(iconTint ?? scheme.onSurfaceVariant)
                    .frame(width: 24)
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(headline).textStyle(.bodyLarge).foregroundStyle(scheme.onSurface)
                if let supporting {
                    Text(supporting).textStyle(.bodyMedium).foregroundStyle(scheme.onSurfaceVariant)
                }
            }
            Spacer(minLength: 0)
            trailing
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .frame(minHeight: 56)
        .opacity(enabled ? 1 : 0.38)
        .contentShape(Rectangle())
    }
}

extension ListRow where Trailing == EmptyView {
    init(icon: String? = nil, iconTint: Color? = nil, headline: String, supporting: String? = nil, enabled: Bool = true) {
        self.icon = icon
        self.iconTint = iconTint
        self.headline = headline
        self.supporting = supporting
        self.enabled = enabled
        trailing = EmptyView()
    }
}

/// A switch row: Material's `ListItem` with a trailing `Switch`.
struct SwitchRow: View {
    let headline: String
    var supporting: String?
    @Binding var isOn: Bool
    @Environment(\.scheme) private var scheme

    var body: some View {
        ListRow(headline: headline, supporting: supporting) {
            Toggle("", isOn: $isOn).labelsHidden().tint(scheme.primary)
        }
        .onTapGesture { isOn.toggle() }
    }
}

/// A form sheet in the place of Material's `AlertDialog`: a heading, what it
/// asks, and its buttons at the foot, in the dialog container colour.
struct DialogSheet<Content: View, Buttons: View>: View {
    let title: String
    @ViewBuilder var content: Content
    @ViewBuilder var buttons: Buttons
    @Environment(\.scheme) private var scheme

    init(_ title: String, @ViewBuilder content: () -> Content, @ViewBuilder buttons: () -> Buttons) {
        self.title = title
        self.content = content()
        self.buttons = buttons()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text(title).textStyle(.headlineSmall).foregroundStyle(scheme.onSurface)
            ScrollView {
                VStack(alignment: .leading, spacing: 12) { content }
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .scrollBounceBehavior(.basedOnSize)
            HStack(spacing: 8) {
                Spacer()
                buttons
            }
        }
        .padding(24)
        .foregroundStyle(scheme.onSurfaceVariant)
        .textStyle(.bodyMedium)
        .presentationDetents([.medium, .large])
        .presentationBackground(scheme.surfaceContainerHigh)
        .presentationCornerRadius(Corner.sheet)
        .environment(\.colorScheme, scheme.colorScheme)
    }
}

/// A text field in the outlined Material style.
struct Field: View {
    let label: String
    @Binding var text: String
    var secret = false
    var keyboard: UIKeyboardType = .default
    var content: UITextContentType?
    var submit: SubmitLabel = .done
    @Environment(\.scheme) private var scheme

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label).textStyle(.bodySmall).foregroundStyle(scheme.onSurfaceVariant)
            Group {
                if secret {
                    SecureField("", text: $text)
                } else {
                    TextField("", text: $text)
                }
            }
            .textStyle(.bodyLarge)
            .foregroundStyle(scheme.onSurface)
            .keyboardType(keyboard)
            .textContentType(content)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .submitLabel(submit)
            .padding(.horizontal, 12)
            .frame(height: 48)
            .overlay(RoundedRectangle(cornerRadius: Corner.tight).strokeBorder(scheme.outline))
        }
    }
}

/// Covers a sheet holding a secret while the app is not frontmost, so the app
/// switcher's snapshot does not keep it — what `FLAG_SECURE` does on Android.
struct SecureContent: ViewModifier {
    @Environment(\.scenePhase) private var phase
    @Environment(\.scheme) private var scheme

    func body(content: Content) -> some View {
        content.overlay {
            if phase != .active {
                scheme.surfaceContainerHigh.ignoresSafeArea()
            }
        }
    }
}

extension View {
    func secureContent() -> some View { modifier(SecureContent()) }
}

/// Where Play on a title lands: the most recently played part-watched episode,
/// else the first unwatched one, else the first — the desktop's `next_up`.
func nextUpIndex(_ playlist: [EpisodeRecord]) -> Int {
    if let resumed = playlist.indices.filter({ playlist[$0].resumeAt != nil }).max(by: { playlist[$0].lastPlayed < playlist[$1].lastPlayed }) {
        return resumed
    }
    return playlist.firstIndex { !$0.watched } ?? 0
}

func formatBytes(_ bytes: UInt64) -> String {
    let value = Double(bytes)
    let gib = 1024.0 * 1024 * 1024
    let mib = 1024.0 * 1024
    if value >= gib { return String(format: "%.1f GiB", value / gib) }
    if value >= mib { return String(format: "%.1f MiB", value / mib) }
    if value >= 1024 { return String(format: "%.1f KiB", value / 1024) }
    return "\(bytes) bytes"
}

/// One line under a download that has not finished: "Running · 120.0 MiB of
/// 1.2 GiB · 2.4 MiB/s", or "Queued" while it waits for a slot.
func downloadStatusLine(_ download: RetainedDownload) -> String {
    var parts = [download.status.prefix(1).uppercased() + download.status.dropFirst()]
    if download.total > 0, download.downloaded > 0 {
        parts.append("\(formatBytes(UInt64(download.downloaded))) of \(formatBytes(UInt64(download.total)))")
    }
    if download.bytesPerSecond > 0, download.status == RetainedDownload.statusRunning {
        parts.append("\(formatBytes(UInt64(download.bytesPerSecond)))/s")
    }
    return parts.joined(separator: " · ")
}

extension EpisodeRecord {
    /// What an episode row is headed with: "1. Mother and Children".
    ///
    /// The provider's name for the episode, else the name the filename gave it,
    /// else "Episode 1". The bare filename only when none of those exist.
    func heading(_ position: Int) -> String {
        let named = providerName ?? (detail != name ? detail : nil)
        switch (named, number) {
        case let (named?, number?): return "\(number). \(named)"
        case let (named?, nil): return named
        case let (nil, number?): return "Episode \(number)"
        case (nil, nil):
            let stem = name.range(of: ".", options: .backwards).map { String(name[..<$0.lowerBound]) } ?? name
            return stem.trimmingCharacters(in: .whitespaces).isEmpty ? "Episode \(position + 1)" : stem
        }
    }
}

extension TitleRecord {
    /// Every episode, in display order — what the player is handed.
    var playlist: [EpisodeRecord] { seasons.flatMap(\.episodes) }
}

/// Colours mixed in sRGB, for the few places a role is blended with another.
func mix(_ from: Color, _ to: Color, _ fraction: Double) -> Color {
    let a = UIColor(from).rgba
    let b = UIColor(to).rgba
    let t = min(max(fraction, 0), 1)
    return Color(
        .sRGB,
        red: a.0 + (b.0 - a.0) * t,
        green: a.1 + (b.1 - a.1) * t,
        blue: a.2 + (b.2 - a.2) * t,
        opacity: a.3 + (b.3 - a.3) * t
    )
}

private extension UIColor {
    var rgba: (Double, Double, Double, Double) {
        var red: CGFloat = 0, green: CGFloat = 0, blue: CGFloat = 0, alpha: CGFloat = 0
        getRed(&red, green: &green, blue: &blue, alpha: &alpha)
        return (red, green, blue, alpha)
    }
}
