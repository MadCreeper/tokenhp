import Foundation
import Security

/// Reads the OAuth token that the Claude Code CLI stores in the macOS Keychain.
///
/// Claude Code saves a generic-password item under the service name
/// `Claude Code-credentials` whose value is JSON shaped like:
/// `{ "claudeAiOauth": { "accessToken": "...", "refreshToken": "...", "expiresAt": <ms> } }`.
public struct ClaudeCredentials: Sendable {
    public let accessToken: String
    public let expiresAt: Date?

    public var isExpired: Bool {
        guard let expiresAt else { return false }
        return expiresAt <= Date()
    }

    public enum LoadError: Error, LocalizedError {
        case notFound
        case accessDenied(OSStatus)
        case malformed

        public var errorDescription: String? {
            switch self {
            case .notFound:
                return "No Claude Code login found. Sign in with the Claude Code CLI first."
            case .accessDenied(let status):
                return "Keychain access denied (status \(status)). Allow access when prompted."
            case .malformed:
                return "Stored Claude Code credentials were not in the expected format."
            }
        }
    }

    private static let service = "Claude Code-credentials"

    public static func load() throws -> ClaudeCredentials {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]

        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)

        switch status {
        case errSecSuccess:
            break
        case errSecItemNotFound:
            throw LoadError.notFound
        default:
            throw LoadError.accessDenied(status)
        }

        guard let data = item as? Data else { throw LoadError.malformed }
        return try parse(data)
    }

    static func parse(_ data: Data) throws -> ClaudeCredentials {
        struct Envelope: Decodable {
            struct OAuth: Decodable {
                let accessToken: String
                let expiresAt: Double?
            }
            let claudeAiOauth: OAuth
        }

        guard let env = try? JSONDecoder().decode(Envelope.self, from: data) else {
            throw LoadError.malformed
        }

        // expiresAt is stored in epoch milliseconds.
        let expiry = env.claudeAiOauth.expiresAt.map {
            Date(timeIntervalSince1970: $0 / 1000)
        }
        return ClaudeCredentials(accessToken: env.claudeAiOauth.accessToken, expiresAt: expiry)
    }
}
