import SwiftUI

// MARK: - Pixel font

private extension Font {
    /// Press Start 2P at a given pt. Falls back to monospaced if the bundled
    /// font hasn't been registered yet (see `FontRegistry`).
    static func pixel(_ size: CGFloat) -> Font {
        .custom(FontRegistry.pixelFontName, size: size)
    }
}

// MARK: - Heart Pixel (continuous fill)

/// A single Minecraft-style heart drawn as a 7×7 pixel grid via Canvas.
///
/// `fill` is the amount of this heart that is "full" (0 = empty, 1 = full).
/// Partial fills drain pixel-by-pixel from the right — at fill 0.6 the left
/// 60% of the body pixels stay red, the right 40% turn dark grey.
public struct HeartPixel: View {
    public let fill: Double
    public let pixelSize: CGFloat

    public init(fill: Double, pixelSize: CGFloat = 3) {
        self.fill = max(0, min(1, fill))
        self.pixelSize = pixelSize
    }

    // 7×7 grid. Codes:
    //   0 = transparent, 1 = outline, 2 = body, 3 = sparkle (top-left highlight)
    private static let pattern: [[Int]] = [
        [0, 1, 1, 0, 1, 1, 0],
        [1, 2, 2, 1, 2, 2, 1],
        [1, 3, 2, 2, 2, 2, 1],
        [1, 2, 2, 2, 2, 2, 1],
        [0, 1, 2, 2, 2, 1, 0],
        [0, 0, 1, 2, 1, 0, 0],
        [0, 0, 0, 1, 0, 0, 0],
    ]
    private static let gridW = 7
    private static let gridH = 7

    // Colors — outline always black; body/sparkle switch on filled vs empty.
    private static let outline      = Color.black
    private static let bodyFull     = Color(red: 0.88, green: 0.11, blue: 0.11)
    private static let sparkleFull  = Color(red: 1.00, green: 0.95, blue: 0.95)
    private static let bodyEmpty    = Color(red: 0.27, green: 0.16, blue: 0.16)   // muted dark red-grey
    private static let sparkleEmpty = Color(red: 0.34, green: 0.22, blue: 0.22)

    // Body pixels live in columns 1...5 (col 0 and 6 are outline-only).
    // Map `fill` onto those 5 body columns so a 1-column drain is actually
    // visible — using all 7 columns made fill 0.86…1.0 look identical.
    private static let firstBodyCol = 1
    private static let bodyColCount = 5

    public var body: some View {
        Canvas { ctx, _ in
            let p = pixelSize
            let filledBodyCols = max(0, min(Self.bodyColCount,
                Int((fill * Double(Self.bodyColCount)).rounded())))
            let lastFilledCol = Self.firstBodyCol + filledBodyCols - 1
            for (y, row) in Self.pattern.enumerated() {
                for (x, code) in row.enumerated() {
                    guard let color = Self.color(code: code, filled: x <= lastFilledCol) else { continue }
                    let rect = CGRect(x: CGFloat(x) * p, y: CGFloat(y) * p, width: p, height: p)
                    ctx.fill(Path(rect), with: .color(color))
                }
            }
        }
        .frame(
            width:  CGFloat(Self.gridW) * pixelSize,
            height: CGFloat(Self.gridH) * pixelSize
        )
    }

    private static func color(code: Int, filled: Bool) -> Color? {
        switch code {
        case 0: return nil
        case 1: return outline
        case 2: return filled ? bodyFull    : bodyEmpty
        case 3: return filled ? sparkleFull : sparkleEmpty
        default: return nil
        }
    }
}

// MARK: - Hearts Bar (10 hearts, continuous drain)

/// Row of 10 hearts that drain pixel-by-pixel as `value` decreases. Value 0.76
/// puts 7 hearts at full, the 8th at ~60%, the rest empty.
public struct MinecraftHeartsBar: View {
    public let value: Double
    public let title: String?
    public let trailing: String?
    public let caption: String?

    public init(value: Double, title: String?, trailing: String?, caption: String?) {
        self.value = max(0, min(1, value))
        self.title = title
        self.trailing = trailing
        self.caption = caption
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack {
                if let title {
                    Text(title)
                        .font(.pixel(8))
                        .foregroundStyle(.primary)
                }
                Spacer()
                if let trailing {
                    Text(trailing)
                        .font(.pixel(8))
                        .foregroundStyle(.secondary)
                }
            }
            HStack(spacing: 2) {
                ForEach(0..<10, id: \.self) { i in
                    HeartPixel(fill: Self.fillFor(heart: i, value: value), pixelSize: 3)
                }
            }
            if let caption {
                Text(caption)
                    .font(.pixel(6))
                    .foregroundStyle(.tertiary)
            }
        }
    }

    /// Fraction of heart `i` that is full given the bar's overall `value`.
    /// Value × 10 = total "hearts of fill" — anything ≥ index+1 fills this
    /// heart completely, anything ≤ index leaves it empty, in between is a
    /// partial fill that drains from the right.
    public static func fillFor(heart index: Int, value: Double) -> Double {
        max(0, min(1, value * 10 - Double(index)))
    }
}

// MARK: - XP Bar (segmented, level centered above)

/// Minecraft-style XP bar: dark slate background with many thin segment
/// dividers, bright green fill, 1-px black border, and the `trailing` value
/// rendered centered above the bar in MC's classic green-with-shadow style.
public struct MinecraftXPBar: View {
    public let value: Double
    public let title: String?
    public let trailing: String?
    public let caption: String?
    public let segmentCount: Int

    public init(
        value: Double,
        title: String?,
        trailing: String?,
        caption: String?,
        segmentCount: Int = 20
    ) {
        self.value = max(0, min(1, value))
        self.title = title
        self.trailing = trailing
        self.caption = caption
        self.segmentCount = max(1, segmentCount)
    }

    static let xpGreen     = Color(red: 0.50, green: 0.92, blue: 0.06)
    static let xpDarkGreen = Color(red: 0.30, green: 0.62, blue: 0.04)
    static let trackDark   = Color(red: 0.08, green: 0.08, blue: 0.08)
    static let segmentInk  = Color.black.opacity(0.55)

    public var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let title {
                Text(title)
                    .font(.pixel(8))
                    .foregroundStyle(.primary)
            }

            // The "level + bar" cluster — fixed-height ZStack so the trailing
            // number can overlap the top edge of the bar (MC's signature look).
            ZStack {
                // Bar pinned to bottom, leaves room above for the level text.
                xpBar
                    .frame(maxHeight: .infinity, alignment: .bottom)

                // Level text pinned to top, centered horizontally. The bar's
                // top edge slides under the bottom row of the text pixels.
                if let trailing {
                    Self.outlinedLevelText(trailing)
                        .frame(maxWidth: .infinity, alignment: .center)
                        .frame(maxHeight: .infinity, alignment: .top)
                }
            }
            .frame(height: 22)

            if let caption {
                Text(caption)
                    .font(.pixel(6))
                    .foregroundStyle(.tertiary)
            }
        }
    }

    /// Pixel-font level number with a 4-direction black outline so it stays
    /// readable on top of the bright green fill (a single drop shadow gets
    /// lost against same-color background).
    @ViewBuilder
    private static func outlinedLevelText(_ s: String) -> some View {
        let label = Text(s).font(.pixel(11))
        ZStack {
            label.foregroundStyle(.black).offset(x:  1, y:  0)
            label.foregroundStyle(.black).offset(x: -1, y:  0)
            label.foregroundStyle(.black).offset(x:  0, y:  1)
            label.foregroundStyle(.black).offset(x:  0, y: -1)
            label.foregroundStyle(xpGreen)
        }
    }

    private var xpBar: some View {
        Canvas { ctx, size in
            // Track
            ctx.fill(Path(CGRect(origin: .zero, size: size)), with: .color(Self.trackDark))

            // Green fill — bright top half, darker bottom half for depth.
            let fillW = size.width * value
            if fillW > 0 {
                let half = size.height * 0.55
                ctx.fill(
                    Path(CGRect(x: 0, y: 0, width: fillW, height: half)),
                    with: .color(Self.xpGreen)
                )
                ctx.fill(
                    Path(CGRect(x: 0, y: half, width: fillW, height: size.height - half)),
                    with: .color(Self.xpDarkGreen)
                )
            }

            // Segment dividers
            let segW = size.width / Double(segmentCount)
            for i in 1..<segmentCount {
                let x = segW * Double(i)
                ctx.fill(
                    Path(CGRect(x: x, y: 0, width: 1, height: size.height)),
                    with: .color(Self.segmentInk)
                )
            }
        }
        .frame(height: 11)
        .clipShape(RoundedRectangle(cornerRadius: 2))
        .overlay(
            RoundedRectangle(cornerRadius: 2)
                .strokeBorder(Color.black, lineWidth: 2)
        )
    }
}

// MARK: - Themes

/// Minecraft hearts — for HP / quota / "remaining" semantics.
public struct MinecraftHeartsTheme: HealthBarTheme {
    public init() { FontRegistry.registerBundledFonts() }
    public let id = "minecraft-hearts"
    public let displayName = "Minecraft · Hearts"

    public func color(for value: Double) -> Color { Color(red: 0.88, green: 0.11, blue: 0.11) }

    public func makeBar(
        value: Double,
        title: String?,
        trailing: String?,
        caption: String?,
        context: BarContext
    ) -> AnyView {
        AnyView(MinecraftHeartsBar(
            value: value, title: title, trailing: trailing, caption: caption
        ))
    }
}

/// Minecraft XP bar — for magnitude / activity semantics (local breakdown).
public struct MinecraftXPTheme: HealthBarTheme {
    public init() { FontRegistry.registerBundledFonts() }
    public let id = "minecraft-xp"
    public let displayName = "Minecraft · XP"

    public func color(for value: Double) -> Color { MinecraftXPBar.xpGreen }

    public func makeBar(
        value: Double,
        title: String?,
        trailing: String?,
        caption: String?,
        context: BarContext
    ) -> AnyView {
        AnyView(MinecraftXPBar(
            value: value, title: title, trailing: trailing, caption: caption
        ))
    }
}
