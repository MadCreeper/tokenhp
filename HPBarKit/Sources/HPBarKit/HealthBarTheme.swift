import SwiftUI

/// Visual theme for `HealthBar`. Conform to this to ship custom looks
/// (different color ramps, bar shapes, heights). The fill is a function of
/// the *remaining* value (1 = full, 0 = empty).
///
/// Override per view tree with `.healthBarTheme(MyTheme())`; the default is
/// ``DefaultTheme`` (the classic green→yellow→red ramp).
public protocol HealthBarTheme: Sendable {
    var id: String { get }
    var displayName: String { get }
    var barHeight: CGFloat { get }
    var cornerRadius: CGFloat { get }
    var trackColor: Color { get }

    /// Base color for a given remaining value (0...1).
    func color(for value: Double) -> Color
    /// The style painted into the bar. Defaults to a vertical gradient of `color(for:)`.
    func fillStyle(for value: Double) -> AnyShapeStyle

    /// Build the entire bar view. Default implementation renders the standard
    /// rectangle. Override in custom themes (e.g. hearts, XP bars) to draw
    /// something completely different.
    @MainActor func makeBar(
        value: Double,
        title: String?,
        trailing: String?,
        caption: String?,
        context: BarContext
    ) -> AnyView
}

public extension HealthBarTheme {
    var barHeight: CGFloat { 10 }
    var cornerRadius: CGFloat { 3 }
    var trackColor: Color { Color.black.opacity(0.25) }

    func fillStyle(for value: Double) -> AnyShapeStyle {
        let base = color(for: value)
        return AnyShapeStyle(
            LinearGradient(
                colors: [base.opacity(0.75), base],
                startPoint: .top,
                endPoint: .bottom
            )
        )
    }

    @MainActor func makeBar(
        value: Double,
        title: String?,
        trailing: String?,
        caption: String?,
        context: BarContext
    ) -> AnyView {
        AnyView(StandardBarView(
            value: value, title: title, trailing: trailing,
            caption: caption, context: context, theme: self
        ))
    }
}

/// The built-in classic theme: a continuous green→yellow→red ramp that drains
/// as quota is consumed.
public struct DefaultTheme: HealthBarTheme {
    public init() {}
    public let id = "default"
    public let displayName = "Classic"

    public func color(for value: Double) -> Color {
        HealthBarStyle.color(for: value)
    }
}

/// A neutral single-color theme for bars that represent *magnitude*
/// (e.g. relative token volumes) rather than "remaining quota". Fill width
/// already encodes value; using one color avoids the misleading
/// "red = bad" signal of the draining theme.
public struct NeutralTheme: HealthBarTheme {
    public init() {}
    public let id = "neutral"
    public let displayName = "Neutral"

    public func color(for value: Double) -> Color {
        Color.accentColor
    }
}

// MARK: - Environment hook

private struct HealthBarThemeKey: EnvironmentKey {
    static let defaultValue: any HealthBarTheme = DefaultTheme()
}

public extension EnvironmentValues {
    var healthBarTheme: any HealthBarTheme {
        get { self[HealthBarThemeKey.self] }
        set { self[HealthBarThemeKey.self] = newValue }
    }
}

public extension View {
    /// Apply a `HealthBarTheme` to every `HealthBar` in this view tree.
    func healthBarTheme(_ theme: any HealthBarTheme) -> some View {
        environment(\.healthBarTheme, theme)
    }
}
