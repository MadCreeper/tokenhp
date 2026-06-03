import Foundation

/// One quota window (e.g. the rolling 5-hour or 7-day subscription limit).
///
/// `title`, `trailing`, and `caption` are display overrides the popover prefers
/// when present — each data source owns how its bar is labelled.
public struct UsageWindow: Sendable, Equatable {
    /// Fraction consumed, clamped to 0...1.
    public let utilization: Double
    /// When this window next resets, if known.
    public let resetsAt: Date?
    public let title: String?
    public let trailing: String?
    public let caption: String?

    public init(
        utilization: Double,
        resetsAt: Date? = nil,
        title: String? = nil,
        trailing: String? = nil,
        caption: String? = nil
    ) {
        self.utilization = max(0, min(1, utilization))
        self.resetsAt = resetsAt
        self.title = title
        self.trailing = trailing
        self.caption = caption
    }

    /// Fraction remaining — what the HP-style draining bar fills to.
    public var remaining: Double { max(0, min(1, 1 - utilization)) }
}

/// Token totals for one exact model id (e.g. `claude-opus-4-8`).
///
/// We track the four kinds the Anthropic API returns separately so the user
/// can see where their usage actually went — output is "real work", cache
/// reads are cheap volume, etc.
public struct ModelUsage: Sendable, Equatable, Identifiable {
    /// Exact model id from the session log (`message.model`).
    public let id: String
    /// Pretty short name for the picker (e.g. "Opus 4.8"). Falls back to id.
    public let displayName: String
    public let input: Int
    public let output: Int
    public let cacheRead: Int
    public let cacheCreate: Int
    /// Dollar cost per token type. `nil` when the model isn't in the
    /// pricing table (e.g. a non-Claude model with no user-provided rates).
    public let cost: ModelCost?

    public init(
        id: String,
        displayName: String,
        input: Int,
        output: Int,
        cacheRead: Int,
        cacheCreate: Int,
        cost: ModelCost? = nil
    ) {
        self.id = id
        self.displayName = displayName
        self.input = input
        self.output = output
        self.cacheRead = cacheRead
        self.cacheCreate = cacheCreate
        self.cost = cost
    }

    /// Sum across all four token kinds — used to rank models by overall volume.
    public var total: Int { input + output + cacheRead + cacheCreate }
    /// Max single bucket; bars fill proportionally to this within the model.
    public var maxComponent: Int { max(max(input, output), max(cacheRead, cacheCreate)) }
}

/// What the UI renders. Different data sources have different shapes:
///   - `.windows` — live subscription quotas (5h / weekly / extra).
///   - `.models`  — per-model token breakdown sorted by total volume.
public struct UsageReport: Sendable, Equatable {
    public enum Body: Sendable, Equatable {
        case windows([UsageWindow])
        case models([ModelUsage])
    }
    public let body: Body
    public let capturedAt: Date
    public let sourceLabel: String

    public init(body: Body, capturedAt: Date = Date(), sourceLabel: String) {
        self.body = body
        self.capturedAt = capturedAt
        self.sourceLabel = sourceLabel
    }
}

/// Anything that can produce a `UsageReport`.
public protocol UsageReporting: Sendable {
    var sourceLabel: String { get }
    func fetch() async throws -> UsageReport
}
