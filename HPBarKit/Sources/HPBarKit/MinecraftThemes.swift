import SwiftUI

// MARK: - Pixel font

public extension Font {
    /// The bundled Minecraft pixel font (Monocraft) at a given pt. Falls back to
    /// the system font if the bundled face hasn't been registered yet
    /// (see `FontRegistry`).
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
                        .font(.pixel(10))
                        .foregroundStyle(.primary)
                }
                Spacer()
                if let trailing {
                    Text(trailing)
                        .font(.pixel(10))
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
                    .font(.pixel(8))
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
                    .font(.pixel(10))
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
                    .font(.pixel(8))
                    .foregroundStyle(.tertiary)
            }
        }
    }

    /// Pixel-font level number with a 4-direction black outline so it stays
    /// readable on top of the bright green fill (a single drop shadow gets
    /// lost against same-color background).
    @ViewBuilder
    private static func outlinedLevelText(_ s: String) -> some View {
        let label = Text(s).font(.pixel(12))
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

// MARK: - GUI chrome (panel + buttons)

/// The classic Minecraft inventory/GUI palette.
/// (#C6C6C6 panel · #8B8B8B widget · #555555 shadow · dark border.)
public enum MinecraftPalette {
    static func gray(_ v: Double) -> Color { Color(red: v, green: v, blue: v) }
    public static let panelFace        = gray(198/255)  // #C6C6C6
    // Buttons carry a slightly cool (blue-grey) stone tint, like the in-game menu.
    public static let buttonFace       = Color(red: 134/255, green: 134/255, blue: 140/255)
    public static let buttonHoverFace  = Color(red: 150/255, green: 152/255, blue: 161/255) // lighter on hover (authentic, not green)
    public static let buttonPressedFace = Color(red: 107/255, green: 107/255, blue: 114/255) // pressed/active
    public static let light            = gray(1)        // raised highlight edge
    public static let shadow           = gray(85/255)   // #555555 recessed edge
    public static let border           = gray(30/255)   // ~#1E1E1E outer frame
    public static let text             = Color.white
    public static let textShadow       = gray(63/255)   // #3F3F3F drop shadow
    public static let panelText        = gray(64/255)   // #404040 labels ON the gray panel
    public static let panelTextDim     = gray(106/255)  // secondary labels on the panel
}

/// A pixel-beveled rectangle — the building block for Minecraft panels and
/// buttons. `raised` draws a 3D-up bevel (light top-left, dark bottom-right);
/// `false` recesses it (used for pressed/active widgets).
public struct MinecraftBevel: View {
    public let face: Color
    public let raised: Bool
    public let unit: CGFloat
    public let textured: Bool
    public init(face: Color, raised: Bool = true, unit: CGFloat = 3, textured: Bool = false) {
        self.face = face; self.raised = raised; self.unit = unit; self.textured = textured
    }
    public var body: some View {
        Canvas { ctx, size in
            let u = unit, w = size.width, h = size.height
            let hi = raised ? MinecraftPalette.light : MinecraftPalette.shadow
            let lo = raised ? MinecraftPalette.shadow : MinecraftPalette.light
            func fill(_ r: CGRect, _ c: Color) { ctx.fill(Path(r), with: .color(c)) }
            // Dark outer frame, then the beveled face inside it.
            fill(CGRect(x: 0, y: 0, width: w, height: h), MinecraftPalette.border)
            let iw = max(0, w - 2 * u), ih = max(0, h - 2 * u)
            let inner = CGRect(x: u, y: u, width: iw, height: ih)
            fill(inner, face)
            if textured { Self.drawStone(ctx, inner) }
            fill(CGRect(x: u, y: u, width: iw, height: u), hi)             // top
            fill(CGRect(x: u, y: u, width: u, height: ih), hi)            // left
            fill(CGRect(x: u, y: h - 2 * u, width: iw, height: u), lo)    // bottom
            fill(CGRect(x: w - 2 * u, y: u, width: u, height: ih), lo)    // right
        }
    }

    /// Subtle horizontal stone grain, like the in-game menu buttons: short
    /// horizontal streaks (a few px long, 2px tall) scattered sparsely over a
    /// very faint sheen. Keyed off pixel coordinates so it stays put on redraw.
    private static func drawStone(_ ctx: GraphicsContext, _ r: CGRect) {
        ctx.fill(Path(r), with: .linearGradient(
            Gradient(colors: [Color.white.opacity(0.04), Color.clear, Color.black.opacity(0.06)]),
            startPoint: CGPoint(x: r.minX, y: r.minY),
            endPoint: CGPoint(x: r.minX, y: r.maxY)))
        let rowStep: CGFloat = 3      // gap between streak rows
        let segStep: CGFloat = 7      // horizontal stride between candidate streaks
        let streakH: CGFloat = 2
        var y = r.minY + 1
        while y < r.maxY - 1 {
            var x = r.minX
            while x < r.maxX {
                let key = (Int(x) &* 49157) ^ (Int(y) &* 98317)
                // ~3/8 of slots get a streak — sparse and subtle.
                switch key & 7 {
                case 0, 1, 2:
                    let len = CGFloat(3 + ((key >> 3) & 3))   // 3…6 px
                    let d: Double = key & 1 == 0 ? -0.07 : 0.05
                    let rect = CGRect(x: x, y: y, width: min(len, r.maxX - x), height: streakH)
                    ctx.fill(Path(rect), with: .color(d < 0 ? .black.opacity(-d) : .white.opacity(d)))
                default:
                    break
                }
                x += segStep
            }
            y += rowStep
        }
    }
}

public extension View {
    /// Use this view as a raised Minecraft GUI panel (the gray inventory look).
    func minecraftPanel(unit: CGFloat = 3) -> some View {
        background(MinecraftBevel(face: MinecraftPalette.panelFace, raised: true, unit: unit))
    }

    /// White pixel text with the classic 1px hard drop-shadow.
    func minecraftText() -> some View {
        foregroundStyle(MinecraftPalette.text)
            .shadow(color: MinecraftPalette.textShadow, radius: 0, x: 1, y: 1)
    }
}

/// A Minecraft GUI button. `selected` keeps it visually pressed-in (for the
/// active tab); hovering lightens the face the way vanilla buttons do.
public struct MinecraftButtonStyle: ButtonStyle {
    public let selected: Bool
    public let unit: CGFloat
    public init(selected: Bool = false, unit: CGFloat = 2) {
        self.selected = selected; self.unit = unit
    }
    public func makeBody(configuration: Configuration) -> some View {
        MinecraftButtonSurface(pressed: configuration.isPressed || selected, unit: unit) {
            configuration.label
        }
    }
}

/// One choice in a `MinecraftDropdown`.
public struct MinecraftDropdownOption<ID: Hashable>: Identifiable {
    public let id: ID
    public let label: String
    public init(id: ID, label: String) { self.id = id; self.label = label }
}

/// An in-game-style expandable menu: a stone button showing the current value
/// that expands a stacked list of stone buttons inline below it (the active one
/// shown pressed-in). No native macOS menu — pure Minecraft.
public struct MinecraftDropdown<ID: Hashable>: View {
    public let current: String
    public let options: [MinecraftDropdownOption<ID>]
    public let selected: ID
    public let onSelect: (ID) -> Void
    @State private var expanded = false

    public init(
        current: String,
        options: [MinecraftDropdownOption<ID>],
        selected: ID,
        onSelect: @escaping (ID) -> Void
    ) {
        self.current = current
        self.options = options
        self.selected = selected
        self.onSelect = onSelect
    }

    public var body: some View {
        VStack(spacing: 4) {
            Button { expanded.toggle() } label: {
                HStack(spacing: 6) {
                    Text(current).font(.pixel(11)).lineLimit(1)
                    Spacer(minLength: 4)
                    Image(systemName: expanded ? "chevron.up" : "chevron.down").font(.pixel(8))
                }
                .frame(maxWidth: .infinity)
            }
            .buttonStyle(MinecraftButtonStyle())

            if expanded {
                VStack(spacing: 4) {
                    ForEach(options) { opt in
                        Button { onSelect(opt.id); expanded = false } label: {
                            Text(opt.label).font(.pixel(11)).frame(maxWidth: .infinity)
                        }
                        .buttonStyle(MinecraftButtonStyle(selected: opt.id == selected))
                    }
                }
            }
        }
    }
}

private struct MinecraftButtonSurface<Label: View>: View {
    let pressed: Bool
    let unit: CGFloat
    @ViewBuilder let label: Label
    @State private var hovering = false

    private var face: Color {
        if pressed { return MinecraftPalette.buttonPressedFace }
        return hovering ? MinecraftPalette.buttonHoverFace : MinecraftPalette.buttonFace
    }

    var body: some View {
        label
            .minecraftText()
            .padding(.horizontal, 9)
            .padding(.vertical, 5)
            .background(MinecraftBevel(face: face, raised: !pressed, unit: unit, textured: true))
            .contentShape(Rectangle())
            .onHover { hovering = $0 }
            .animation(nil, value: hovering)
    }
}
