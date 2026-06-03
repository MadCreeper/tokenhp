import SwiftUI

/// Color math for the classic look: a continuous green → yellow → red ramp.
/// `value` is the *remaining* fraction (1 = full health, 0 = empty), so the
/// bar starts green and bleeds toward red as quota is consumed.
public enum HealthBarStyle {
    // RGB stops for the ramp.
    private static let green:  (Double, Double, Double) = (0.24, 0.80, 0.28)
    private static let yellow: (Double, Double, Double) = (0.95, 0.79, 0.30)
    private static let red:    (Double, Double, Double) = (0.91, 0.29, 0.24)

    public static func color(for value: Double) -> Color {
        let v = max(0, min(1, value))
        // Upper half blends yellow→green, lower half blends red→yellow.
        let rgb: (Double, Double, Double) = v >= 0.5
            ? mix(yellow, green, (v - 0.5) / 0.5)
            : mix(red, yellow, v / 0.5)
        return Color(red: rgb.0, green: rgb.1, blue: rgb.2)
    }

    public static func gradient(for value: Double) -> LinearGradient {
        let base = color(for: value)
        return LinearGradient(
            colors: [base.opacity(0.75), base],
            startPoint: .top,
            endPoint: .bottom
        )
    }

    private static func mix(
        _ a: (Double, Double, Double),
        _ b: (Double, Double, Double),
        _ t: Double
    ) -> (Double, Double, Double) {
        let t = max(0, min(1, t))
        return (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, a.2 + (b.2 - a.2) * t)
    }
}
