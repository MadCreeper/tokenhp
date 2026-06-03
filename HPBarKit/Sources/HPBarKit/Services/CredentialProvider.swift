import Foundation

/// Caches Claude Code credentials in memory so the Keychain is read only when
/// the cached token is missing or near expiry — not on every poll.
///
/// Claude Code rotates its OAuth token every few hours and rewrites its
/// Keychain item, which resets the item's ACL and can re-trigger the macOS
/// permission prompt. Reading on every 180s poll multiplied those prompts;
/// caching collapses Keychain access to roughly once per token-refresh cycle.
public actor CredentialProvider {
    private var cached: ClaudeCredentials?
    private let margin: TimeInterval
    private let loader: @Sendable () throws -> ClaudeCredentials

    /// - Parameters:
    ///   - margin: re-read the Keychain once the cached token is within this
    ///     many seconds of expiry.
    ///   - loader: how to read fresh credentials (defaults to the Keychain).
    public init(
        margin: TimeInterval = 120,
        loader: @escaping @Sendable () throws -> ClaudeCredentials = { try ClaudeCredentials.load() }
    ) {
        self.margin = margin
        self.loader = loader
    }

    /// A usable credential. Reads the Keychain only when the cache is empty or
    /// the cached token is within `margin` seconds of expiry.
    public func credentials() throws -> ClaudeCredentials {
        if let cached, !isStale(cached) { return cached }
        let fresh = try loader()
        cached = fresh
        return fresh
    }

    /// Drop the cache so the next `credentials()` call re-reads the Keychain.
    public func invalidate() { cached = nil }

    private func isStale(_ c: ClaudeCredentials) -> Bool {
        guard let expiresAt = c.expiresAt else { return false }
        return expiresAt.timeIntervalSinceNow <= margin
    }
}
