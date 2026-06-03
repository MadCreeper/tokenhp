import Testing
import Foundation
@testable import HPBarKit

@Suite("Pricing")
struct PricingTests {
    @Test("bundled JSON loads and includes Anthropic + new entries")
    func bundledTableLoads() {
        let p = Pricing.loaded()
        // Spot-check a handful — full table is the JSON itself.
        #expect(p.price(for: "claude-opus-4-8")    != nil)
        #expect(p.price(for: "claude-sonnet-4-6")  != nil)
        #expect(p.price(for: "claude-haiku-4-5")   != nil)
        #expect(p.price(for: "kimi-k2.6")          != nil)
        #expect(p.price(for: "minimax-m2.7")       != nil)
        #expect(p.price(for: "doubao-seed-code")   != nil)
    }

    @Test("Anthropic Opus 4.x family carries the current post-cut prices")
    func opusPricesMatchAnthropic() {
        let p = Pricing.loaded().price(for: "claude-opus-4-8")
        #expect(p?.input         == 5)
        #expect(p?.output        == 25)
        #expect(p?.cacheRead     == 0.50)
        #expect(p?.cacheCreate   == 6.25)
        #expect(p?.cacheCreate1h == 10)
    }

    @Test("date suffix is stripped on lookup fallback")
    func dateSuffixFallback() {
        let p = Pricing.loaded()
        // claude-haiku-4-5-20251001 must resolve to the same row as claude-haiku-4-5
        #expect(p.price(for: "claude-haiku-4-5-20251001") == p.price(for: "claude-haiku-4-5"))
        #expect(p.price(for: "claude-3-5-sonnet-20241022") == p.price(for: "claude-3-5-sonnet"))
    }

    @Test("unknown model has no price")
    func unknownIsNil() {
        #expect(Pricing.empty.price(for: "totally-unknown") == nil)
        #expect(Pricing.loaded().price(for: "totally-unknown") == nil)
    }

    @Test("cost math splits cache writes across 5m/1h rates")
    func costMathSplitsCacheTTLs() throws {
        let p = Pricing(table: [
            "x": ModelPrice(input: 10, output: 20, cacheRead: 1, cacheCreate: 12.5, cacheCreate1h: 20)
        ])
        let c = try #require(p.cost(
            for: "x",
            input: 1_000_000,         // × $10/M = $10
            output: 100_000,          // × $20/M = $2
            cacheRead: 500_000,       // × $1/M  = $0.5
            cacheCreate5m: 200_000,   // × $12.5/M = $2.5
            cacheCreate1h: 300_000    // × $20/M   = $6
        ))
        #expect(c.input        == 10)
        #expect(c.output       == 2)
        #expect(c.cacheRead    == 0.5)
        #expect(c.cacheCreate  == 8.5)   // 2.5 + 6
        #expect(c.total        == 21)
    }

    @Test("when cache_create_1h is omitted, 1h writes use the 5m rate")
    func cacheCreate1hFallsBackToBaseRate() throws {
        let p = Pricing(table: [
            "x": ModelPrice(input: 1, output: 1, cacheRead: 1, cacheCreate: 5, cacheCreate1h: nil)
        ])
        let c = try #require(p.cost(
            for: "x",
            input: 0, output: 0, cacheRead: 0,
            cacheCreate5m: 1_000_000,
            cacheCreate1h: 1_000_000
        ))
        // Both buckets at $5/M = $10 combined
        #expect(c.cacheCreate == 10)
    }

    @Test("ModelPrice round-trips through JSON with snake_case keys")
    func modelPriceJSONRoundTrip() throws {
        let json = #"""
        {"input": 5, "output": 25, "cache_read": 0.5, "cache_create": 6.25, "cache_create_1h": 10}
        """#
        let decoded = try JSONDecoder().decode(ModelPrice.self, from: Data(json.utf8))
        #expect(decoded.input         == 5)
        #expect(decoded.cacheRead     == 0.5)
        #expect(decoded.cacheCreate   == 6.25)
        #expect(decoded.cacheCreate1h == 10)
    }
}
