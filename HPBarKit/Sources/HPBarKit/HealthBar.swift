import SwiftUI

public enum BarContext: Sendable { case app, widget }

/// A health-bar-shaped UI primitive. The actual rendering is delegated to the
/// active `HealthBarTheme` (set via `.healthBarTheme(_:)`), so a custom theme
/// can replace the rectangle with hearts, an XP bar, or anything else.
public struct HealthBar: View {
    public enum Kind: CaseIterable, Sendable {
        case hp, mp, exp

        public var label: String {
            switch self {
            case .hp: return "5-Hour"
            case .mp: return "Weekly"
            case .exp: return "Monthly"
            }
        }
    }

    @Environment(\.healthBarTheme) private var theme

    public let kind: Kind
    public let value: Double
    public let context: BarContext
    public let title: String?
    public let trailing: String?
    public let caption: String?

    public init(
        kind: Kind,
        value: Double,
        title: String? = nil,
        trailing: String? = nil,
        caption: String? = nil,
        context: BarContext = .app
    ) {
        self.kind = kind
        self.value = max(0, min(1, value))
        self.title = title
        self.trailing = trailing
        self.caption = caption
        self.context = context
    }

    public var body: some View {
        theme.makeBar(
            value: value,
            title: title ?? kind.label,
            trailing: trailing ?? "\(Int(value * 100))%",
            caption: caption,
            context: context
        )
    }
}

/// The standard rectangle render — what `DefaultTheme` and `NeutralTheme` use.
struct StandardBarView: View {
    let value: Double
    let title: String?
    let trailing: String?
    let caption: String?
    let context: BarContext
    let theme: any HealthBarTheme

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(title ?? "")
                    .font(.system(.caption, design: .monospaced, weight: .semibold))
                    .foregroundStyle(.primary)
                Spacer()
                Text(trailing ?? "")
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(.secondary)
            }

            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    RoundedRectangle(cornerRadius: theme.cornerRadius)
                        .fill(theme.trackColor)

                    RoundedRectangle(cornerRadius: theme.cornerRadius)
                        .fill(theme.fillStyle(for: value))
                        .frame(width: max(0, geo.size.width * value))
                        .if(context == .app) { view in
                            view.animation(.easeOut(duration: 0.4), value: value)
                        }
                }
            }
            .frame(height: theme.barHeight)

            if let caption {
                Text(caption)
                    .font(.system(size: 9, design: .monospaced))
                    .foregroundStyle(.tertiary)
            }
        }
    }
}
