import SwiftUI

/// The four buttons the Android client draws, at the shared corner radius.
///
/// The primary button wears the accent ramp; a disabled one keeps a flat
/// disabled fill, because a greyed control that still wears the one strong
/// colour in the window reads as pressable.
enum ButtonKind {
    case accent, tonal, edged, quiet
}

struct ProtonButtonStyle: ButtonStyle {
    let kind: ButtonKind
    var expand = false
    @Environment(\.scheme) private var scheme
    @Environment(\.isEnabled) private var enabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .textStyle(.labelLarge)
            .lineLimit(1)
            .padding(.horizontal, kind == .quiet ? 12 : 24)
            .frame(maxWidth: expand ? .infinity : nil)
            // 40pt drawn, inside the 48pt a finger is given.
            .frame(minHeight: 40)
            .foregroundStyle(foreground)
            .background { fill }
            .overlay {
                if kind == .edged {
                    RoundedRectangle(cornerRadius: Corner.standard)
                        .strokeBorder(enabled ? scheme.outline : scheme.onSurface.opacity(0.12))
                }
            }
            .opacity(configuration.isPressed ? 0.8 : 1)
            .contentShape(Rectangle())
            .padding(.vertical, 4)
    }

    private var foreground: Color {
        guard enabled else { return scheme.onSurface.opacity(0.38) }
        switch kind {
        case .accent: return scheme.onPrimary
        case .tonal: return scheme.onSecondaryContainer
        case .edged, .quiet: return scheme.primary
        }
    }

    @ViewBuilder private var fill: some View {
        let shape = RoundedRectangle(cornerRadius: Corner.standard)
        switch kind {
        case .accent:
            if enabled { shape.fill(scheme.accentGradient) } else { shape.fill(scheme.onSurface.opacity(0.12)) }
        case .tonal:
            shape.fill(enabled ? scheme.secondaryContainer : scheme.onSurface.opacity(0.12))
        case .edged, .quiet:
            Color.clear
        }
    }
}

extension ButtonStyle where Self == ProtonButtonStyle {
    static var accent: ProtonButtonStyle { ProtonButtonStyle(kind: .accent) }
    static var accentWide: ProtonButtonStyle { ProtonButtonStyle(kind: .accent, expand: true) }
    static var tonal: ProtonButtonStyle { ProtonButtonStyle(kind: .tonal) }
    static var edged: ProtonButtonStyle { ProtonButtonStyle(kind: .edged) }
    static var edgedWide: ProtonButtonStyle { ProtonButtonStyle(kind: .edged, expand: true) }
    static var quiet: ProtonButtonStyle { ProtonButtonStyle(kind: .quiet) }
}

/// How far along something is, drawn in the accent.
///
/// `solid` is for the one bar that is deliberately not the accent: a download in
/// flight sits directly under the watch-progress bar of the same row, and two
/// ramps in the same colour stacked two points apart say nothing.
struct AccentProgress: View {
    let progress: Double
    var solid: Color?
    @Environment(\.scheme) private var scheme

    var body: some View {
        GeometryReader { geometry in
            let height = geometry.size.height
            let done = geometry.size.width * min(max(progress, 0), 1)
            ZStack(alignment: .leading) {
                Capsule().fill(scheme.surfaceContainerHighest)
                // Below one bar's width there is nothing to round.
                if done >= height {
                    if let solid {
                        Capsule().fill(solid).frame(width: done)
                    } else {
                        Capsule().fill(scheme.accentGradient).frame(width: done)
                    }
                }
            }
        }
        .frame(height: 4)
    }
}

/// The seek bar, in the accent ramp, with what is already read ahead shaded
/// under the played part: a jump that lands in it costs no fetch.
///
/// `onCommit` fires once, when the finger lifts — a seek per drag step is a
/// block fetch per drag step.
struct AccentTrack: View {
    @Binding var value: Double
    let range: ClosedRange<Double>
    var buffered: Double = 0
    var ticks: [Double] = []
    var onEditing: (Bool) -> Void = { _ in }
    @Environment(\.scheme) private var scheme
    @State private var dragging = false

    var body: some View {
        GeometryReader { geometry in
            let width = geometry.size.width
            let span = max(range.upperBound - range.lowerBound, 0.001)
            let fraction = min(max((value - range.lowerBound) / span, 0), 1)
            ZStack(alignment: .leading) {
                Capsule().fill(Color.white.opacity(0.30)).frame(height: 6)
                if width * buffered >= 6 {
                    Capsule().fill(Color.white.opacity(0.55)).frame(width: width * min(max(buffered, 0), 1), height: 6)
                }
                if width * fraction >= 6 {
                    Capsule().fill(scheme.accentGradient).frame(width: width * fraction, height: 6)
                }
                ForEach(ticks.indices, id: \.self) { index in
                    let tick = min(max(ticks[index], 0), 1)
                    Rectangle().fill(Color.black.opacity(0.6)).frame(width: 2, height: 6)
                        .offset(x: width * tick - 1)
                }
                Circle().fill(scheme.primary)
                    .frame(width: dragging ? 22 : 16, height: dragging ? 22 : 16)
                    .offset(x: width * fraction - (dragging ? 11 : 8))
            }
            .frame(maxHeight: .infinity)
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { drag in
                        if !dragging {
                            dragging = true
                            onEditing(true)
                        }
                        let x = min(max(drag.location.x / max(width, 1), 0), 1)
                        value = range.lowerBound + x * span
                    }
                    .onEnded { _ in
                        dragging = false
                        onEditing(false)
                    }
            )
        }
        .frame(height: 32)
        .accessibilityElement()
        .accessibilityValue(Text(clock(value)))
        .accessibilityAdjustableAction { direction in
            onEditing(true)
            value = min(max(value + (direction == .increment ? 10 : -10), range.lowerBound), range.upperBound)
            onEditing(false)
        }
    }
}

/// Seconds as `m:ss` or `h:mm:ss`.
func clock(_ seconds: Double) -> String {
    let total = Int(max(seconds, 0).rounded(.down))
    let hours = total / 3600
    let minutes = (total % 3600) / 60
    let rest = total % 60
    return hours > 0
        ? String(format: "%d:%02d:%02d", hours, minutes, rest)
        : String(format: "%d:%02d", minutes, rest)
}
