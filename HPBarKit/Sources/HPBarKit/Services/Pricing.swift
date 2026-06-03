import Foundation

/// Per-million-token price for one model, in USD.
///
/// `cacheCreate` covers the default cache-write rate (Anthropic's 5-minute TTL,
/// 1.25× input). `cacheCreate1h` is optional — when set, it applies to the
/// 1-hour TTL writes (2× input for Anthropic models). Non-Claude models that
/// don't have a TTL distinction can omit `cache_create_1h` and the 5m rate
/// will be used for both.
public struct ModelPrice: Codable, Sendable, Equatable {
    public let input: Double
    public let output: Double
    public let cacheRead: Double
    public let cacheCreate: Double
    public let cacheCreate1h: Double?

    public init(
        input: Double,
        output: Double,
        cacheRead: Double,
        cacheCreate: Double,
        cacheCreate1h: Double? = nil
    ) {
        self.input = input
        self.output = output
        self.cacheRead = cacheRead
        self.cacheCreate = cacheCreate
        self.cacheCreate1h = cacheCreate1h
    }

    enum CodingKeys: String, CodingKey {
        case input, output
        case cacheRead = "cache_read"
        case cacheCreate = "cache_create"
        case cacheCreate1h = "cache_create_1h"
    }
}

/// Per-component dollar cost for a `ModelUsage` row.
public struct ModelCost: Sendable, Equatable {
    public let input: Double
    public let output: Double
    public let cacheRead: Double
    public let cacheCreate: Double  // combined 5m + 1h
    public var total: Double { input + output + cacheRead + cacheCreate }
    public init(input: Double, output: Double, cacheRead: Double, cacheCreate: Double) {
        self.input = input; self.output = output
        self.cacheRead = cacheRead; self.cacheCreate = cacheCreate
    }
}

/// Table of model-id → ModelPrice. Built-in entries cover current Anthropic
/// models; users can override or extend (e.g. `kimi-k2.6`) by writing JSON to
/// `~/Library/Application Support/HPBar/pricing.json`:
///
/// ```json
/// {
///   "kimi-k2.6": {
///     "input": 0.50, "output": 2.00,
///     "cache_read": 0.10, "cache_create": 0.60,
///     "cache_create_1h": 0.95   // optional; omit if vendor has no TTL split
///   }
/// }
/// ```
///
/// All values are dollars per million tokens. Anything in the user file
/// overrides the built-in entry of the same id.
public struct Pricing: Sendable {
    public let table: [String: ModelPrice]

    public init(table: [String: ModelPrice]) { self.table = table }

    public func price(for modelId: String) -> ModelPrice? {
        // Fall back to a date-suffix-stripped id so `claude-haiku-4-5-20251001`
        // matches a `claude-haiku-4-5` entry.
        table[modelId] ?? table[Self.stripDateSuffix(modelId)]
    }

    /// Dollar cost split by component. Cache-write tokens are passed as two
    /// buckets because Anthropic charges 5m vs 1h cache writes at different
    /// rates (1.25× vs 2× the base input price).
    public func cost(
        for modelId: String,
        input: Int,
        output: Int,
        cacheRead: Int,
        cacheCreate5m: Int,
        cacheCreate1h: Int
    ) -> ModelCost? {
        guard let p = price(for: modelId) else { return nil }
        func c(_ tokens: Int, _ rate: Double) -> Double {
            Double(tokens) * rate / 1_000_000
        }
        let oneHourRate = p.cacheCreate1h ?? p.cacheCreate
        return ModelCost(
            input:       c(input,         p.input),
            output:      c(output,        p.output),
            cacheRead:   c(cacheRead,     p.cacheRead),
            cacheCreate: c(cacheCreate5m, p.cacheCreate) + c(cacheCreate1h, oneHourRate)
        )
    }

    // MARK: - Loading
    //
    // All prices live in JSON, not Swift. Two sources, merged:
    //   1. Bundled `pricing.json` — defaults shipped with the app
    //      (HPBarKit/Sources/HPBarKit/Resources/pricing.json). Source of
    //      truth for Anthropic models. Updated by app releases.
    //   2. `~/Library/Application Support/HPBar/pricing.json` — user
    //      overrides. Anything here wins over the bundled file. Use it to
    //      add custom models or correct rates without recompiling.

    /// Load the bundled table, then overlay user overrides. Missing or
    /// malformed files are tolerated (bundled missing → empty; user missing
    /// → just bundled).
    public static func loaded() -> Pricing {
        var merged = bundledTable()
        for (k, v) in userOverrides() { merged[k] = v }
        return Pricing(table: merged)
    }

    /// Empty table — useful for tests / DI.
    public static let empty = Pricing(table: [:])

    private static func bundledTable() -> [String: ModelPrice] {
        guard let url = Bundle.module.url(forResource: "pricing", withExtension: "json"),
              let data = try? Data(contentsOf: url),
              let table = try? JSONDecoder().decode([String: ModelPrice].self, from: data)
        else { return [:] }
        return table
    }

    private static func userOverrides() -> [String: ModelPrice] {
        guard let url = overrideURL(),
              let data = try? Data(contentsOf: url),
              let table = try? JSONDecoder().decode([String: ModelPrice].self, from: data)
        else { return [:] }
        return table
    }

    private static func stripDateSuffix(_ id: String) -> String {
        var parts = id.split(separator: "-").map(String.init)
        while let last = parts.last, last.count >= 6, last.allSatisfy(\.isNumber) {
            parts.removeLast()
        }
        return parts.joined(separator: "-")
    }

    private static func overrideURL() -> URL? {
        let fm = FileManager.default
        guard let supp = try? fm.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: false
        ) else { return nil }
        return supp.appendingPathComponent("HPBar/pricing.json")
    }
}
