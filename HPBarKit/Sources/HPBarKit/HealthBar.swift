import SwiftUI

public enum BarContext { case app, widget }

public struct HealthBar: View {
    public enum Kind: CaseIterable {
        case hp, mp, exp

        public var label: String {
            switch self {
            case .hp: return "5-Hour"
            case .mp: return "Weekly"
            case .exp: return "Monthly"
            }
        }
    }

    public let kind: Kind
    public let value: Double
    public let context: BarContext
    /// Optional override for the label shown on the left.
    public let title: String?
    /// Optional override for the right-hand text (defaults to remaining %).
    public let trailing: String?
    /// Optional small caption under the bar (e.g. reset time).
    public let caption: String?

    @Environment(\.healthBarTheme) private var theme

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
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(title ?? kind.label)
                    .font(.system(.caption, design: .monospaced, weight: .semibold))
                    .foregroundStyle(.primary)
                Spacer()
                Text(trailing ?? "\(Int(value * 100))%")
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
