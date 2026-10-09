import Observation
import SwiftUI

/// The palette in force, shared across every view.
///
/// The choice is stored by Rust and resolved by Rust — the same flavours, the
/// same accent pairings and the same contrast rule the desktop client uses — so
/// what lives here is only the resolved answer and a way to replace it when the
/// viewer picks something else.
@MainActor
@Observable
final class AppearanceState {
    static let shared = AppearanceState()

    private(set) var palette: PaletteRecord?

    /// Before the first read, and if it fails, the palette this app shipped
    /// with — a theme file that cannot be read must never be a reason the
    /// library does not open.
    var scheme: Scheme { Scheme(palette ?? protonFallback) }

    /// The palette read before the first frame, from
    /// `NativeRuntime.initialPalette()`.
    func seed(_ palette: PaletteRecord?) {
        if let palette { self.palette = palette }
    }

    /// Read the stored choice through the engine. A no-op once `seed` has run.
    func load() async {
        if palette != nil { return }
        if let read = try? await Task.detached(operation: { try NativeRuntime.blockingEngine().palette() }).value {
            palette = read
        }
    }

    /// Read the stored choice again, after sync rewrote it.
    func reload() async {
        if let read = try? await Task.detached(operation: { try NativeRuntime.blockingEngine().palette() }).value {
            palette = read
        }
    }

    /// Repaint immediately, before the write that stores the choice lands.
    func apply(_ palette: PaletteRecord) {
        self.palette = palette
    }
}

/// One resolved palette as the roles the views draw with — Material's names,
/// so a view ported from the Android client asks for the same role.
///
/// Every role is answered from the palette, and the mapping is Android's
/// `schemeOf` line for line; `SchemeRolesTests` pins it.
struct Scheme: Equatable {
    let light: Bool
    let primary: Color
    let onPrimary: Color
    let primaryContainer: Color
    let onPrimaryContainer: Color
    let inversePrimary: Color
    let secondary: Color
    let onSecondary: Color
    let secondaryContainer: Color
    let onSecondaryContainer: Color
    let tertiary: Color
    let onTertiary: Color
    let tertiaryContainer: Color
    let onTertiaryContainer: Color
    let background: Color
    let onBackground: Color
    let surface: Color
    let onSurface: Color
    let surfaceVariant: Color
    let onSurfaceVariant: Color
    let inverseSurface: Color
    let inverseOnSurface: Color
    let error: Color
    let onError: Color
    let errorContainer: Color
    let onErrorContainer: Color
    let outline: Color
    let outlineVariant: Color
    let scrim: Color
    let surfaceBright: Color
    let surfaceDim: Color
    let surfaceContainerLowest: Color
    let surfaceContainerLow: Color
    let surfaceContainer: Color
    let surfaceContainerHigh: Color
    let surfaceContainerHighest: Color

    init(_ palette: PaletteRecord) {
        light = palette.light
        primary = argb(palette.accent)
        onPrimary = argb(palette.onAccent)
        primaryContainer = argb(palette.accentDim)
        onPrimaryContainer = argb(palette.text)
        // Readable *on* `inverseSurface`, which is the ink colour: `accentDim`
        // is the accent taken towards the page, so it inverts with the rest.
        inversePrimary = argb(palette.accentDim)
        secondary = argb(palette.accentAlt)
        onSecondary = argb(palette.onAccent)
        // The accent taken back towards the page, not the card's hover shade:
        // it fills every tonal button.
        secondaryContainer = argb(palette.accentDim)
        onSecondaryContainer = argb(palette.text)
        // The palette has two hues, not three. Tertiary takes the gradient
        // partner rather than inventing a third that no flavour chose.
        tertiary = argb(palette.accentAlt)
        onTertiary = argb(palette.onAccent)
        tertiaryContainer = argb(palette.accentDim)
        onTertiaryContainer = argb(palette.text)
        background = argb(palette.background)
        onBackground = argb(palette.text)
        surface = argb(palette.surface)
        onSurface = argb(palette.text)
        surfaceVariant = argb(palette.card)
        onSurfaceVariant = argb(palette.muted)
        // Genuinely inverted, both ways round: on a dark flavour this is a pale
        // snackbar with dark ink, on Latte a dark one with pale ink.
        inverseSurface = argb(palette.text)
        inverseOnSurface = argb(palette.background)
        error = argb(palette.danger)
        onError = argb(palette.background)
        errorContainer = argb(palette.dangerDim)
        onErrorContainer = argb(palette.text)
        // `outline` is a border meant to be seen, `outlineVariant` a divider
        // meant to be barely there.
        outline = argb(palette.muted)
        outlineVariant = argb(palette.border)
        scrim = .black
        surfaceBright = argb(palette.elevated)
        surfaceDim = argb(palette.background)
        // The containment ladder, in the order the palette already runs.
        surfaceContainerLowest = argb(palette.sunken)
        surfaceContainerLow = argb(palette.surface)
        surfaceContainer = argb(palette.card)
        surfaceContainerHigh = argb(palette.cardHover)
        surfaceContainerHighest = argb(palette.elevated)
    }

    var colorScheme: ColorScheme { light ? .light : .dark }

    /// The accent, as a gradient when there is one to draw. With gradients
    /// off, `Palette::resolve` sets `accent_alt` equal to `accent`, so this
    /// collapses to a flat fill on its own.
    var accentGradient: LinearGradient {
        LinearGradient(colors: [primary, secondary], startPoint: .leading, endPoint: .trailing)
    }
}

/// A packed `0xAARRGGBB` from the bridge, as SwiftUI wants it.
func argb(_ value: UInt32) -> Color {
    Color(
        .sRGB,
        red: Double((value >> 16) & 0xFF) / 255,
        green: Double((value >> 8) & 0xFF) / 255,
        blue: Double(value & 0xFF) / 255,
        opacity: Double((value >> 24) & 0xFF) / 255
    )
}

/// The shipped default, spelled out for the case where Rust cannot be asked.
/// The values are `pstr_core::appearance::Palette::PROTON`.
let protonFallback = PaletteRecord(
    background: 0xFF0E_0E12,
    surface: 0xFF17_171D,
    sunken: 0xFF0A_0A0D,
    card: 0xFF1E_1E26,
    cardHover: 0xFF2A_2A35,
    border: 0xFF26_2630,
    text: 0xFFEA_EAF0,
    muted: 0xFF8E_8E9C,
    accent: 0xFF7D_4DFF,
    accentAlt: 0xFF3F_6FE0,
    accentDim: 0xFF53_33AD,
    onAccent: 0xFFEA_EAF0,
    danger: 0xFFE0_5561,
    elevated: 0xFF33_3340,
    dangerDim: 0xFF7F_2F37,
    light: false
)

private struct SchemeKey: EnvironmentKey {
    static let defaultValue = Scheme(protonFallback)
}

extension EnvironmentValues {
    var scheme: Scheme {
        get { self[SchemeKey.self] }
        set { self[SchemeKey.self] = newValue }
    }
}

/// One corner radius, which is the desktop client's corner radius: 8 for
/// everything, 4 for a badge over artwork, 16 for a sheet.
enum Corner {
    static let standard: CGFloat = 8
    static let tight: CGFloat = 4
    static let sheet: CGFloat = 16
}
