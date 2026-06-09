import SwiftUI
import AppKit
import HPBarKit

@main
struct HPBarApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    var body: some Scene {
        // No SwiftUI windows — the whole UI is a status-item popover driven by
        // AppDelegate. Settings is the canonical "no visible window" scene.
        Settings { EmptyView() }
    }
}

/// A borderless panel that can still take keyboard/mouse focus (borderless
/// windows can't become key by default), so the popover's SwiftUI controls work.
final class PopoverPanel: NSPanel {
    override var canBecomeKey: Bool { true }
}

/// Hosts the popover content and reports its ideal size up to the AppDelegate,
/// which repositions the panel — keeping the top edge pinned so it grows down.
private struct PanelSizeKey: PreferenceKey {
    static let defaultValue = CGSize.zero
    static func reduce(value: inout CGSize, nextValue: () -> CGSize) { value = nextValue() }
}

private struct PanelRoot: View {
    @Bindable var model: UsageViewModel
    let onResize: @Sendable (CGSize) -> Void

    var body: some View {
        MenuBarPopover(model: model)
            .frame(width: 340)
            .fixedSize(horizontal: false, vertical: true)
            .background(GeometryReader { geo in
                Color.clear.preference(key: PanelSizeKey.self, value: geo.size)
            })
            .onPreferenceChange(PanelSizeKey.self) { onResize($0) }
    }
}

/// Owns the menu-bar status item and a custom borderless panel anchored beneath
/// it. We position the panel ourselves and pin its TOP edge, so content-height
/// changes (tabs, the model dropdown, theme switch) grow it *downward* — no jump
/// and no clipping, whether the menu bar is shown or hidden (fullscreen). It sits
/// a few px below the icon so the revealed menu bar never covers it.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private let model = UsageViewModel()
    private var statusItem: NSStatusItem?
    private var panel: PopoverPanel?
    private var clickMonitor: Any?
    /// Fixed screen-Y of the panel's top edge while it's open (so it grows down).
    private var anchorTop: CGFloat?
    private let topGap: CGFloat = 6

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Pull the bundled pixel font into the process so the Minecraft theme's
        // .font(.custom(...)) can resolve it.
        FontRegistry.registerBundledFonts()
        setUpStatusItem()
        setUpPanel()
        model.startPolling()
    }

    private func setUpStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        if let button = item.button {
            let icon = NSImage(systemSymbolName: "heart.fill", accessibilityDescription: "HP Bar")
            icon?.isTemplate = true
            button.image = icon
            button.action = #selector(toggle)
            button.target = self
        }
        statusItem = item
    }

    private func setUpPanel() {
        let panel = PopoverPanel(
            contentRect: NSRect(x: 0, y: 0, width: 340, height: 240),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered, defer: true
        )
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.level = .popUpMenu
        panel.isMovable = false
        panel.hidesOnDeactivate = false
        panel.animationBehavior = .none
        // Allow it to appear over fullscreen apps and on every space.
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]

        let host = NSHostingView(rootView: PanelRoot(model: model) { [weak self] size in
            MainActor.assumeIsolated { self?.resize(to: size) }
        })
        host.autoresizingMask = [.width, .height]
        panel.contentView = host
        self.panel = panel
    }

    @objc private func toggle() {
        (panel?.isVisible ?? false) ? close() : open()
    }

    private func open() {
        guard let panel, let button = statusItem?.button, let buttonWindow = button.window
        else { return }
        // Anchor the panel's top just below the status item (lowered by topGap).
        let buttonScreen = buttonWindow.convertToScreen(button.convert(button.bounds, to: nil))
        anchorTop = buttonScreen.minY - topGap

        // Size to the SwiftUI content up front so it opens in the right place.
        panel.contentView?.layoutSubtreeIfNeeded()
        let size = panel.contentView?.fittingSize ?? NSSize(width: 340, height: 240)
        place(size: size, rightEdge: buttonScreen.maxX, screen: buttonWindow.screen)

        panel.makeKeyAndOrderFront(nil)
        Task { await model.refresh() }

        if clickMonitor == nil {
            clickMonitor = NSEvent.addGlobalMonitorForEvents(
                matching: [.leftMouseDown, .rightMouseDown]
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.close() }
            }
        }
    }

    /// Re-fit the panel when the content height changes, keeping top + right
    /// edges fixed so it only grows/shrinks downward.
    private func resize(to size: CGSize) {
        guard let panel, panel.isVisible, size.height > 1 else { return }
        place(size: size, rightEdge: panel.frame.maxX, screen: panel.screen)
    }

    private func place(size: CGSize, rightEdge: CGFloat, screen: NSScreen?) {
        guard let panel, let top = anchorTop else { return }
        var x = rightEdge - size.width
        if let visible = (screen ?? NSScreen.main)?.visibleFrame {
            x = min(x, visible.maxX - size.width - 8)
            x = max(x, visible.minX + 8)
        }
        panel.setFrame(NSRect(x: x, y: top - size.height, width: size.width, height: size.height),
                       display: true)
        panel.invalidateShadow()
    }

    private func close() {
        panel?.orderOut(nil)
        if let clickMonitor { NSEvent.removeMonitor(clickMonitor) }
        clickMonitor = nil
    }
}

struct MenuBarPopover: View {
    @Bindable var model: UsageViewModel
    @AppStorage("visualTheme") private var visualThemeId: String = "classic"

    private var isMinecraft: Bool { visualThemeId == "minecraft" }

    /// Theme for quota (drain) bars — Live tab.
    private var quotaTheme: any HealthBarTheme {
        isMinecraft ? MinecraftHeartsTheme() : DefaultTheme()
    }

    /// Theme for magnitude (fill) bars — Local breakdown.
    private var magnitudeTheme: any HealthBarTheme {
        isMinecraft ? MinecraftXPTheme() : NeutralTheme()
    }

    var body: some View {
        VStack(spacing: 14) {
            header
            sourcePicker
            content
        }
        .padding()
        // The panel window is transparent, so each theme paints its own chrome:
        // Minecraft → the gray inventory panel (pinned light so on-panel labels
        // stay dark); Classic → a rounded translucent material like the old
        // system popover.
        .if(isMinecraft) { $0.minecraftPanel().environment(\.colorScheme, .light) }
        .if(!isMinecraft) {
            $0.background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
                .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color.primary.opacity(0.08)))
        }
    }

    // MARK: - Header

    @ViewBuilder
    private var header: some View {
        HStack(spacing: 6) {
            Image(systemName: "heart.fill").foregroundStyle(.red)
            if isMinecraft {
                Text("Claude Quota")
                    .font(.pixel(13))
                    .foregroundStyle(MinecraftPalette.panelText)
            } else {
                Text("Claude Quota").font(.headline)
            }
            Spacer()
            if model.isLoading { ProgressView().controlSize(.small) }
            themeControl
            refreshButton
        }
    }

    @ViewBuilder
    private var themeControl: some View {
        if isMinecraft {
            // Two themes → a Minecraft-style cycle button (no native menu).
            Button { visualThemeId = "classic" } label: {
                Image(systemName: "paintbrush.fill").font(.pixel(11))
            }
            .buttonStyle(MinecraftButtonStyle())
        } else {
            Menu {
                Picker("Theme", selection: $visualThemeId) {
                    Text("Classic").tag("classic")
                    Text("Minecraft").tag("minecraft")
                }
            } label: {
                Image(systemName: "paintbrush.fill")
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
        }
    }

    @ViewBuilder
    private var refreshButton: some View {
        if isMinecraft {
            Button { Task { await model.refresh() } } label: {
                Image(systemName: "arrow.clockwise").font(.pixel(11))
            }
            .buttonStyle(MinecraftButtonStyle())
        } else {
            Button { Task { await model.refresh() } } label: {
                Image(systemName: "arrow.clockwise")
            }
            .buttonStyle(.borderless)
        }
    }

    /// A Minecraft-styled segmented control: one stone button per case, the
    /// active one shown pressed-in. Reused for the source tabs and the
    /// local-window picker.
    @ViewBuilder
    private func mcSegment<T: CaseIterable & Identifiable & Equatable>(
        _ selection: Binding<T>, title: @escaping (T) -> String
    ) -> some View where T.AllCases: RandomAccessCollection {
        HStack(spacing: 6) {
            ForEach(T.allCases) { item in
                Button { selection.wrappedValue = item } label: {
                    Text(title(item)).font(.pixel(11)).frame(maxWidth: .infinity)
                }
                .buttonStyle(MinecraftButtonStyle(selected: selection.wrappedValue == item))
            }
        }
    }

    // MARK: - Source tabs

    @ViewBuilder
    private var sourcePicker: some View {
        if isMinecraft {
            mcSegment($model.source) { $0.title }
        } else {
            Picker("Source", selection: $model.source) {
                ForEach(UsageViewModel.Source.allCases) { source in
                    Text(source.title).tag(source)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
        }
    }

    @ViewBuilder
    private var content: some View {
        if let report = model.report {
            switch report.body {
            case .windows(let windows):
                VStack(spacing: 12) {
                    ForEach(Array(windows.enumerated()), id: \.offset) { _, w in
                        windowBar(w)
                    }
                }
                .healthBarTheme(quotaTheme)
            case .models(let models):
                modelView(models)
            }
            if let error = model.errorMessage {
                refreshErrorBanner(error)
            }
            footer(report)
        } else if let error = model.errorMessage {
            Text(error)
                .font(.caption)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .frame(maxHeight: .infinity)
        } else {
            ProgressView().frame(maxHeight: .infinity)
        }
    }

    // MARK: - Windows (live quota)

    private func windowBar(_ window: UsageWindow) -> some View {
        let used = Int((window.utilization * 100).rounded())
        let left = Int((window.remaining * 100).rounded())
        return HealthBar(
            kind: .hp,                      // kind only affects default label; we override below
            value: window.remaining,
            title: window.title,
            trailing: window.trailing ?? "\(used)% used · \(left)% left",
            caption: window.caption ?? window.resetsAt.map(Self.resetCaption)
        )
    }

    /// "resets 2:50 PM" today; "resets Jun 3 11:00 PM" otherwise.
    private static func resetCaption(_ date: Date) -> String {
        let time = date.formatted(date: .omitted, time: .shortened)
        if Calendar.current.isDateInToday(date) { return "resets \(time)" }
        let day = date.formatted(.dateTime.month(.abbreviated).day())
        return "resets \(day) \(time)"
    }

    // MARK: - Models (local breakdown)

    @ViewBuilder
    private func modelView(_ models: [ModelUsage]) -> some View {
        let current = models.first { $0.id == model.selectedLocalModelId } ?? models.first
        VStack(spacing: 8) {
            if isMinecraft {
                mcSegment($model.localWindow) { $0.title }
                MinecraftDropdown(
                    current: current?.displayName ?? "Model",
                    options: models.map { MinecraftDropdownOption(id: $0.id, label: $0.displayName) },
                    selected: current?.id ?? "",
                    onSelect: { model.selectedLocalModelId = $0 }
                )
            } else {
                Picker("Window", selection: $model.localWindow) {
                    ForEach(UsageViewModel.LocalWindow.allCases) { w in
                        Text(w.title).tag(w)
                    }
                }
                .pickerStyle(.segmented)
                .labelsHidden()

                Picker("Model", selection: $model.selectedLocalModelId) {
                    ForEach(models) { m in
                        Text(m.displayName).tag(Optional(m.id))
                    }
                }
                .labelsHidden()
            }
        }
        if let current {
            HStack(spacing: 6) {
                Text(current.id)
                    .font(.system(size: 9, design: .monospaced))
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer()
                if let cost = current.cost {
                    Text(Self.formatDollars(cost.total))
                        .font(.system(size: 11, weight: .semibold, design: .monospaced))
                        .foregroundStyle(.primary)
                }
            }
            VStack(spacing: 10) {
                breakdownBar("Input",   current.input,       current.cost?.input,       current.maxComponent)
                breakdownBar("Output",  current.output,      current.cost?.output,      current.maxComponent)
                breakdownBar("Cache R", current.cacheRead,   current.cost?.cacheRead,   current.maxComponent)
                breakdownBar("Cache W", current.cacheCreate, current.cost?.cacheCreate, current.maxComponent)
            }
            .healthBarTheme(magnitudeTheme)
        } else {
            Text("No model activity in this window.")
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    private func breakdownBar(_ label: String, _ tokens: Int, _ dollars: Double?, _ peak: Int) -> some View {
        let frac = peak > 0 ? Double(tokens) / Double(peak) : 0
        let trailing = dollars.map { "\(Self.formatTokens(tokens)) · \(Self.formatDollars($0))" }
            ?? Self.formatTokens(tokens)
        return HealthBar(
            kind: .hp,
            value: frac,
            title: label,
            trailing: trailing
        )
    }

    private static func formatTokens(_ n: Int) -> String {
        if n < 1_000 { return "\(n)" }
        if n < 1_000_000 { return "\(Int((Double(n) / 1_000).rounded()))k" }
        if n < 1_000_000_000 { return String(format: "%.1fM", Double(n) / 1_000_000) }
        return String(format: "%.2fB", Double(n) / 1_000_000_000)
    }

    private static func formatDollars(_ d: Double) -> String {
        if d == 0 { return "$0" }
        if d < 0.01 { return "<$0.01" }
        if d < 1_000 { return String(format: "$%.2f", d) }
        if d < 1_000_000 { return String(format: "$%.1fk", d / 1_000) }
        return String(format: "$%.2fM", d / 1_000_000)
    }

    /// Shown when a refresh fails but we still have a prior report on screen —
    /// otherwise the failure is invisible (stale bars, no feedback).
    private func refreshErrorBanner(_ message: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 5) {
            Image(systemName: "exclamationmark.triangle.fill")
            Text(message)
            Spacer(minLength: 0)
        }
        .font(.caption2)
        .foregroundStyle(.orange)
        .multilineTextAlignment(.leading)
    }

    // MARK: - Footer

    private func footer(_ report: UsageReport) -> some View {
        let updated = "Updated \(report.capturedAt.formatted(date: .omitted, time: .shortened))"
        return VStack(spacing: 3) {
            if isMinecraft {
                Text(report.sourceLabel)
                    .font(.pixel(10))
                    .foregroundStyle(MinecraftPalette.panelText)
                Text(updated)
                    .font(.pixel(9))
                    .foregroundStyle(MinecraftPalette.panelTextDim)
            } else {
                Text(report.sourceLabel)
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(.secondary)
                Text(updated)
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            }
        }
    }
}
