import Foundation
import Observation

/// Drives the menu-bar UI: holds the current report, refreshes on a timer,
/// and lets the user switch between the live quota and local activity sources.
@MainActor
@Observable
public final class UsageViewModel {
    public enum Source: String, CaseIterable, Identifiable {
        case live, local
        public var id: String { rawValue }
        public var title: String {
            switch self {
            case .live: return "Live quota"
            case .local: return "Local activity"
            }
        }
    }

    /// Rolling-window choices for the Local-activity view.
    public enum LocalWindow: String, CaseIterable, Identifiable {
        case day, week, month
        public var id: String { rawValue }
        public var title: String {
            switch self {
            case .day:   return "24h"
            case .week:  return "7d"
            case .month: return "30d"
            }
        }
        public var seconds: TimeInterval {
            switch self {
            case .day:   return 86_400
            case .week:  return 7 * 86_400
            case .month: return 30 * 86_400
            }
        }
        public var caption: String {
            switch self {
            case .day:   return "last 24h"
            case .week:  return "last 7 days"
            case .month: return "last 30 days"
            }
        }
    }

    public private(set) var report: UsageReport?
    public private(set) var errorMessage: String?
    public private(set) var isLoading = false
    /// Which model id the Local-activity view is currently displaying.
    /// Defaults to the highest-volume model in the latest report.
    public var selectedLocalModelId: String?

    public var source: Source {
        didSet {
            guard source != oldValue else { return }
            // Show whatever we cached last time on this tab — feels instant —
            // then quietly refresh in the background.
            report = cache[currentKey]
            errorMessage = nil
            Task { await refresh() }
        }
    }

    public var localWindow: LocalWindow {
        didSet {
            guard localWindow != oldValue else { return }
            local = Self.makeLocal(window: localWindow, pricing: pricing)
            if source == .local {
                report = cache[currentKey]
                Task { await refresh() }
            }
        }
    }

    /// Cache of the last successful fetch per (source, window). Held in memory
    /// for the app's lifetime — switching tabs shows the cached snapshot
    /// instantly while a background refresh runs.
    private enum CacheKey: Hashable {
        case live
        case local(LocalWindow)
    }
    private var cache: [CacheKey: UsageReport] = [:]
    private var currentKey: CacheKey {
        switch source {
        case .live: return .live
        case .local: return .local(localWindow)
        }
    }

    private let live: any UsageReporting
    private var local: any UsageReporting
    private let pricing: Pricing
    private let pollInterval: Duration
    private var pollTask: Task<Void, Never>?

    public init(
        source: Source = .live,
        live: any UsageReporting = OAuthUsageDataSource(),
        localWindow: LocalWindow = .day,
        pricing: Pricing = .loaded(),
        pollInterval: Duration = .seconds(1800)
    ) {
        self.source = source
        self.live = live
        self.localWindow = localWindow
        self.pricing = pricing
        self.local = Self.makeLocal(window: localWindow, pricing: pricing)
        self.pollInterval = pollInterval
    }

    private static func makeLocal(window: LocalWindow, pricing: Pricing) -> LocalStatsDataSource {
        LocalStatsDataSource(
            window: window.seconds,
            windowLabel: window.caption,
            pricing: pricing
        )
    }

    private var current: any UsageReporting {
        source == .live ? live : local
    }

    public func refresh() async {
        isLoading = true
        defer { isLoading = false }
        // Snapshot what we're fetching so a fast tab/window switch can't be
        // clobbered by an in-flight result that no longer matches the view.
        let startedKey = currentKey
        let snapshotSource = current
        do {
            let fresh = try await snapshotSource.fetch()
            cache[startedKey] = fresh
            guard startedKey == currentKey else { return }
            report = fresh
            syncLocalSelection(fresh)
            errorMessage = nil
        } catch {
            guard startedKey == currentKey else { return }
            errorMessage = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// Keep `selectedLocalModelId` valid: snap to the highest-volume model when
    /// no selection exists or the previous one is no longer in the report.
    private func syncLocalSelection(_ report: UsageReport) {
        guard case .models(let models) = report.body else { return }
        if let sel = selectedLocalModelId, models.contains(where: { $0.id == sel }) { return }
        selectedLocalModelId = models.first?.id
    }

    /// Start the background poll loop. Sleeps first, then refreshes on the
    /// interval — the immediate refresh is driven by the popover opening, so
    /// this avoids a duplicate fetch on launch.
    public func startPolling() {
        guard pollTask == nil else { return }
        pollTask = Task { [weak self] in
            guard let self else { return }
            while !Task.isCancelled {
                try? await Task.sleep(for: self.pollInterval)
                guard !Task.isCancelled else { return }
                await self.refresh()
            }
        }
    }

    public func stopPolling() {
        pollTask?.cancel()
        pollTask = nil
    }
}
