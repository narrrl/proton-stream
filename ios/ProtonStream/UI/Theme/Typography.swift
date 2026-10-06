import SwiftUI

/// The type ramp, which is the desktop client's type ramp, under Material's
/// names so a view ported from the Android client asks for the same style.
///
/// Inter, Regular and SemiBold only, as on the desktop and on Android: a
/// `Medium` role resolves to Regular. The rungs, largest first:
/// 26 / 20 / 18 / 17 / 15 / 14 / 13 / 12 / 11. `TypeRampTests` pins them.
enum TypeRole: CaseIterable {
    case displayLarge, displayMedium, displaySmall
    case headlineLarge, headlineMedium, headlineSmall
    case titleLarge, titleMedium, titleSmall
    case bodyLarge, bodyMedium, bodySmall
    case labelLarge, labelMedium, labelSmall

    var size: CGFloat {
        switch self {
        case .displayLarge, .displayMedium, .headlineLarge, .headlineMedium: Ramp.display
        case .displaySmall, .titleLarge: Ramp.title
        case .headlineSmall: Ramp.heading
        case .titleMedium: Ramp.section
        case .titleSmall: Ramp.subhead
        case .bodyLarge: Ramp.body
        case .bodyMedium, .labelLarge: Ramp.label
        case .bodySmall, .labelMedium: Ramp.caption
        case .labelSmall: Ramp.micro
        }
    }

    var semibold: Bool {
        switch self {
        case .displayLarge, .displayMedium, .displaySmall, .headlineLarge, .headlineMedium, .headlineSmall, .titleLarge:
            true
        default:
            false
        }
    }

    /// The Dynamic Type style each rung scales with.
    var relative: Font.TextStyle {
        switch size {
        case Ramp.display: .title
        case Ramp.title: .title2
        case Ramp.heading: .title3
        case Ramp.section: .headline
        case Ramp.subhead: .subheadline
        case Ramp.body: .body
        case Ramp.label: .callout
        case Ramp.caption: .footnote
        default: .caption
        }
    }

    /// Line height is derived rather than named per style: 1.35 of the size,
    /// as on Android. Inter's own leading is about 1.21, so the rest is spacing.
    var lineSpacing: CGFloat { size * (Ramp.lineHeight - 1.21) }
}

enum Ramp {
    static let display: CGFloat = 26
    static let title: CGFloat = 20
    static let heading: CGFloat = 18
    static let section: CGFloat = 17
    static let subhead: CGFloat = 15
    static let body: CGFloat = 14
    static let label: CGFloat = 13
    static let caption: CGFloat = 12
    static let micro: CGFloat = 11
    static let lineHeight: CGFloat = 1.35
}

extension Font {
    static func inter(_ style: TypeRole, semibold: Bool? = nil) -> Font {
        .custom((semibold ?? style.semibold) ? "Inter-SemiBold" : "Inter-Regular", size: style.size, relativeTo: style.relative)
    }

    static func inter(size: CGFloat, semibold: Bool = false) -> Font {
        .custom(semibold ? "Inter-SemiBold" : "Inter-Regular", size: size)
    }
}

extension View {
    /// One rung of the ramp. `semibold` overrides the style's own weight, the
    /// counterpart of Compose's `fontWeight = FontWeight.SemiBold`.
    func textStyle(_ style: TypeRole, semibold: Bool? = nil) -> some View {
        font(.inter(style, semibold: semibold)).lineSpacing(style.lineSpacing)
    }
}
