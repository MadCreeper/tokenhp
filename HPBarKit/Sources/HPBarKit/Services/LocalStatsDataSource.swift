import Foundation

/// Tracks per-model token usage by scanning Claude Code's local session
/// transcripts (`~/.claude/projects/**/ *.jsonl`).
///
/// Each assistant message carries the exact `message.model` id and a
/// `message.usage` breakdown (input / cache_creation / cache_read / output).
/// We aggregate every model id seen — Claude and non-Claude alike — so the
/// picker shows whatever the user actually ran.
public struct LocalStatsDataSource: UsageReporting {
    public var sourceLabel: String { "Local model usage · \(windowLabel)" }

    private let projectsDir: URL
    private let window: TimeInterval
    private let windowLabel: String
    private let pricing: Pricing
    private let now: () -> Date

    public init(
        projectsDir: URL = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".claude/projects"),
        window: TimeInterval = 86_400,
        windowLabel: String = "last 24h",
        pricing: Pricing = .loaded(),
        now: @escaping () -> Date = Date.init
    ) {
        self.projectsDir = projectsDir
        self.window = window
        self.windowLabel = windowLabel
        self.pricing = pricing
        self.now = now
    }

    public enum StatsError: Error, LocalizedError {
        case noSessions
        case noActivity
        public var errorDescription: String? {
            switch self {
            case .noSessions: return "No local Claude Code sessions found under ~/.claude/projects."
            case .noActivity: return "No model activity in the last 24 hours."
            }
        }
    }

    public func fetch() async throws -> UsageReport {
        let files = sessionFiles()
        guard !files.isEmpty else { throw StatsError.noSessions }

        let reference = now()
        var totals: [String: Totals] = [:]
        let decoder = JSONDecoder()

        for file in files {
            guard let text = try? String(contentsOf: file, encoding: .utf8) else { continue }
            for line in text.split(separator: "\n", omittingEmptySubsequences: true) {
                // ~half the lines are queue-operation / user / file-snapshot /
                // attachment etc. A cheap substring check skips them before
                // we pay for JSONDecoder.
                guard line.contains(Self.assistantMarker) else { continue }
                guard let data = line.data(using: .utf8),
                      let row = try? decoder.decode(Row.self, from: data),
                      row.type == "assistant",
                      let stamp = row.timestamp,
                      let date = Self.parseDate(stamp)
                else { continue }
                let age = reference.timeIntervalSince(date)
                guard age >= 0, age < window else { continue }
                guard let msg = row.message,
                      let model = msg.model,
                      !model.hasPrefix("<")   // skip placeholder ids like "<synthetic>"
                else { continue }
                totals[model, default: Totals()].add(msg.usage)
            }
        }

        guard !totals.isEmpty else { throw StatsError.noActivity }

        let models = totals
            .map { id, t in
                ModelUsage(
                    id: id,
                    displayName: Self.displayName(of: id),
                    input: t.input,
                    output: t.output,
                    cacheRead: t.cacheRead,
                    cacheCreate: t.cacheCreate,
                    cost: pricing.cost(
                        for: id,
                        input: t.input,
                        output: t.output,
                        cacheRead: t.cacheRead,
                        cacheCreate5m: t.cacheCreate5m,
                        cacheCreate1h: t.cacheCreate1h
                    )
                )
            }
            .sorted { $0.total > $1.total }

        return UsageReport(body: .models(models), sourceLabel: sourceLabel)
    }

    // MARK: - Helpers

    private func sessionFiles() -> [URL] {
        guard let enumerator = FileManager.default.enumerator(
            at: projectsDir,
            includingPropertiesForKeys: nil,
            options: [.skipsHiddenFiles]
        ) else { return [] }
        return enumerator.compactMap { $0 as? URL }.filter { $0.pathExtension == "jsonl" }
    }

    private struct Totals {
        var input = 0
        var output = 0
        var cacheRead = 0
        var cacheCreate5m = 0
        var cacheCreate1h = 0
        var cacheCreate: Int { cacheCreate5m + cacheCreate1h }

        mutating func add(_ u: Row.Usage?) {
            input     += u?.input_tokens ?? 0
            output    += u?.output_tokens ?? 0
            cacheRead += u?.cache_read_input_tokens ?? 0
            let cc5m = u?.cache_creation?.ephemeral_5m_input_tokens ?? 0
            let cc1h = u?.cache_creation?.ephemeral_1h_input_tokens ?? 0
            let ccTotal = u?.cache_creation_input_tokens ?? 0
            // Older log rows may omit the breakdown but populate the total.
            // Default missing breakdowns to the cheaper 5m bucket.
            if cc5m + cc1h == 0 && ccTotal > 0 {
                cacheCreate5m += ccTotal
            } else {
                cacheCreate5m += cc5m
                cacheCreate1h += cc1h
            }
        }
    }

    private struct Row: Decodable {
        let type: String?
        let timestamp: String?
        let message: Message?
        struct Message: Decodable {
            let model: String?
            let usage: Usage?
        }
        struct Usage: Decodable {
            let input_tokens: Int?
            let cache_creation_input_tokens: Int?
            let cache_read_input_tokens: Int?
            let output_tokens: Int?
            let cache_creation: CacheCreation?
            struct CacheCreation: Decodable {
                let ephemeral_5m_input_tokens: Int?
                let ephemeral_1h_input_tokens: Int?
            }
        }
    }

    /// Turn a model id like `claude-opus-4-8` or `claude-3-5-sonnet-20240620`
    /// into a short label like "Opus 4.8" / "Sonnet 3.5". Non-Claude ids
    /// (e.g. "gpt-4o") pass through verbatim.
    static func displayName(of id: String) -> String {
        let lower = id.lowercased()
        guard lower.hasPrefix("claude") else { return id }

        var parts = id.split(separator: "-").map(String.init)
        // Drop a trailing date stamp like "20251001".
        while let last = parts.last, last.count >= 6, last.allSatisfy(\.isNumber) {
            parts.removeLast()
        }
        // Drop the leading "claude" token.
        if parts.first?.lowercased() == "claude" { parts.removeFirst() }

        let families: Set<String> = ["opus", "sonnet", "haiku"]
        guard let famIdx = parts.firstIndex(where: { families.contains($0.lowercased()) }) else {
            return id
        }
        let family = parts[famIdx].capitalized
        // Version = every numeric part anywhere else (works for both
        // `opus-4-8` and `3-5-sonnet` orderings).
        let version = parts.enumerated()
            .filter { $0.offset != famIdx && $0.element.allSatisfy(\.isNumber) }
            .map(\.element)
            .joined(separator: ".")
        return version.isEmpty ? family : "\(family) \(version)"
    }

    private static let assistantMarker = "\"type\":\"assistant\""

    private static let isoFractional: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()
    private static let iso: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime]
        return f
    }()
    static func parseDate(_ s: String) -> Date? {
        isoFractional.date(from: s) ?? iso.date(from: s)
    }
}
