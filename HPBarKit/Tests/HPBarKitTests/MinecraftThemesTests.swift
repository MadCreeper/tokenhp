import Testing
@testable import HPBarKit

@Suite("MinecraftHeartsBar.fillFor")
struct MinecraftHeartsBarTests {
    private typealias Bar = MinecraftHeartsBar

    @Test("full value → all 10 hearts full")
    func fullHealth() {
        for i in 0..<10 {
            #expect(Bar.fillFor(heart: i, value: 1.0) == 1.0)
        }
    }

    @Test("zero value → all 10 hearts empty")
    func zeroHealth() {
        for i in 0..<10 {
            #expect(Bar.fillFor(heart: i, value: 0.0) == 0.0)
        }
    }

    @Test("0.5 → 5 full + 5 empty")
    func halfPoint() {
        let fills = (0..<10).map { Bar.fillFor(heart: $0, value: 0.5) }
        #expect(fills.prefix(5).allSatisfy { $0 == 1.0 })
        #expect(fills.suffix(5).allSatisfy { $0 == 0.0 })
    }

    @Test("0.76 → 7 full + 1 partial(0.6) + 2 empty (user's worked example)")
    func sevenSixty() {
        let fills = (0..<10).map { Bar.fillFor(heart: $0, value: 0.76) }
        #expect(fills[0...6].allSatisfy { $0 == 1.0 })
        #expect(abs(fills[7] - 0.6) < 1e-9)
        #expect(fills[8] == 0.0)
        #expect(fills[9] == 0.0)
    }

    @Test("0.17 → 1 full + 1 at 0.7 + 8 empty (low health)")
    func lowHealth() {
        let fills = (0..<10).map { Bar.fillFor(heart: $0, value: 0.17) }
        #expect(fills[0] == 1.0)
        #expect(abs(fills[1] - 0.7) < 1e-9)
        #expect(fills[2...].allSatisfy { $0 == 0.0 })
    }

    @Test("values clamp — ≥1 saturates, ≤0 empties")
    func clamping() {
        for i in 0..<10 {
            #expect(Bar.fillFor(heart: i, value: 5.0)  == 1.0)
            #expect(Bar.fillFor(heart: i, value: -1.0) == 0.0)
        }
    }
}
