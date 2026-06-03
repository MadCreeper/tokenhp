import Testing
import Foundation
@testable import HPBarKit

@Suite("LocalStatsDataSource helpers")
struct LocalStatsDataSourceTests {
    // MARK: - displayName(of:)

    @Test("Claude id with version-after-family → 'Family X.Y'")
    func claudeNewFormat() {
        #expect(LocalStatsDataSource.displayName(of: "claude-opus-4-8")   == "Opus 4.8")
        #expect(LocalStatsDataSource.displayName(of: "claude-sonnet-4-6") == "Sonnet 4.6")
        #expect(LocalStatsDataSource.displayName(of: "claude-haiku-4-5")  == "Haiku 4.5")
    }

    @Test("Claude id with date suffix strips the 8-digit tail")
    func claudeWithDateSuffix() {
        #expect(LocalStatsDataSource.displayName(of: "claude-haiku-4-5-20251001") == "Haiku 4.5")
        #expect(LocalStatsDataSource.displayName(of: "claude-3-5-sonnet-20241022") == "Sonnet 3.5")
    }

    @Test("Legacy Claude format (version-before-family)")
    func claudeLegacyFormat() {
        #expect(LocalStatsDataSource.displayName(of: "claude-3-5-sonnet") == "Sonnet 3.5")
        #expect(LocalStatsDataSource.displayName(of: "claude-3-5-haiku")  == "Haiku 3.5")
    }

    @Test("Non-Claude ids pass through verbatim")
    func nonClaudePassesThrough() {
        #expect(LocalStatsDataSource.displayName(of: "kimi-k2.6")        == "kimi-k2.6")
        #expect(LocalStatsDataSource.displayName(of: "deepseek-v4-pro")  == "deepseek-v4-pro")
        #expect(LocalStatsDataSource.displayName(of: "gpt-4o")           == "gpt-4o")
        #expect(LocalStatsDataSource.displayName(of: "doubao-seed-code") == "doubao-seed-code")
    }

    // MARK: - parseDate(_:)

    @Test("ISO8601 with fractional seconds (the real endpoint format)")
    func parsesFractionalSeconds() {
        let d = LocalStatsDataSource.parseDate("2026-05-27T10:50:00.795109+00:00")
        #expect(d != nil)
    }

    @Test("ISO8601 without fractional seconds")
    func parsesPlainISO() {
        let d = LocalStatsDataSource.parseDate("2026-05-27T10:50:00Z")
        #expect(d != nil)
    }

    @Test("garbage strings return nil")
    func rejectsGarbage() {
        #expect(LocalStatsDataSource.parseDate("not a date") == nil)
        #expect(LocalStatsDataSource.parseDate("") == nil)
    }
}

@Suite("UsageWindow")
struct UsageWindowTests {
    @Test("utilization is clamped to 0...1")
    func utilizationClamps() {
        #expect(UsageWindow(utilization: -0.5).utilization == 0)
        #expect(UsageWindow(utilization: 1.7).utilization  == 1)
        #expect(UsageWindow(utilization: 0.3).utilization  == 0.3)
    }

    @Test("remaining = 1 - utilization, also clamped")
    func remainingIsComplement() {
        #expect(UsageWindow(utilization: 0).remaining == 1)
        #expect(UsageWindow(utilization: 1).remaining == 0)
        #expect(abs(UsageWindow(utilization: 0.83).remaining - 0.17) < 1e-9)
    }
}
