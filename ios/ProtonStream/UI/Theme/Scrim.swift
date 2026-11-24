import SwiftUI

/// `pstr_core::appearance::SCRIM_STOPS`: the suite's one fade, opacity from the
/// solid end to the clear one at nine evenly spaced stops.
///
/// Eased (`1 - smoothstep`), flat at both ends, so a scrim over artwork has no
/// line where it starts or stops. The desktop draws the same curve, and a test
/// there pins these numbers to it. Android's copy is `ui/theme/Scrim.kt`.
private let scrimStops: [Double] = [1, 0.957, 0.844, 0.684, 0.5, 0.316, 0.156, 0.043, 0]

/// A fade in `color` from `start` (opaque) to `end` (clear), over the
/// fractions `from` to `to` of the way between them.
func scrim(
    _ color: Color,
    from: Double = 0,
    to: Double = 1,
    start: UnitPoint = .bottom,
    end: UnitPoint = .top
) -> LinearGradient {
    let last = Double(scrimStops.count - 1)
    let stops = scrimStops.enumerated().map { index, opacity in
        Gradient.Stop(color: color.opacity(opacity), location: from + (to - from) * Double(index) / last)
    }
    return LinearGradient(stops: stops, startPoint: start, endPoint: end)
}
