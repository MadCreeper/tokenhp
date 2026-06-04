import SwiftUI
import HPBarKit

@main
struct HPBarApp: App {
    @State private var model = UsageViewModel()

    init() {
        // Pull bundled pixel font into the process so .font(.custom(...))
        // can resolve it in the Minecraft theme.
        FontRegistry.registerBundledFonts()
    }

    var body: some Scene {
        MenuBarExtra("HP Bar", systemImage: "bolt.fill") {
            MenuBarPopover(model: model)
                .frame(width: 340)
                .fixedSize(horizontal: false, vertical: true)
                .onAppear {
                    model.startPolling()
                    Task { await model.refresh() }
                }
        }
        .menuBarExtraStyle(.window)
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
    }

    private var header: some View {
        HStack(spacing: 6) {
            Image(systemName: "bolt.fill").foregroundStyle(.yellow)
            Text("Claude Quota").font(.headline)
            Spacer()
            if model.isLoading { ProgressView().controlSize(.small) }
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
            Button {
                Task { await model.refresh() }
            } label: {
                Image(systemName: "arrow.clockwise")
            }
            .buttonStyle(.borderless)
        }
    }

    private var sourcePicker: some View {
        Picker("Source", selection: $model.source) {
            ForEach(UsageViewModel.Source.allCases) { source in
                Text(source.title).tag(source)
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
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

    // MARK: - Footer

    private func footer(_ report: UsageReport) -> some View {
        VStack(spacing: 2) {
            Text(report.sourceLabel)
                .font(.caption2.weight(.semibold))
                .foregroundStyle(.secondary)
            Text("Updated \(report.capturedAt.formatted(date: .omitted, time: .shortened))")
                .font(.caption2)
                .foregroundStyle(.tertiary)
        }
    }
}
