import Foundation

/// Fetches live subscription quota from Anthropic's undocumented OAuth usage
/// endpoint — the same data that powers Claude Code's `/usage` view.
///
///     GET https://api.anthropic.com/api/oauth/usage
///     Authorization: Bearer <oauth access token>
///     anthropic-beta: oauth-2025-04-20
///     User-Agent: claude-code/<version>
///
/// The `User-Agent` is required; without it the request lands in an
/// aggressively rate-limited bucket and returns 429s.
public struct OAuthUsageDataSource: UsageReporting {
    public let sourceLabel = "Live quota"

    private let userAgent: String
    private let session: URLSession
    private let credentials: CredentialProvider

    public init(
        claudeCodeVersion: String = "2.1.152",
        session: URLSession = .shared,
        credentials: CredentialProvider = CredentialProvider()
    ) {
        self.userAgent = "claude-code/\(claudeCodeVersion)"
        self.session = session
        self.credentials = credentials
    }

    public enum FetchError: Error, LocalizedError {
        case tokenExpired
        case unauthorized
        case rateLimited
        case server(Int)
        case decoding

        public var errorDescription: String? {
            switch self {
            case .tokenExpired, .unauthorized:
                return "Login expired. Open Claude Code to refresh, then retry."
            case .rateLimited:
                return "Rate limited by Anthropic. Try again shortly."
            case .server(let code):
                return "Usage endpoint returned HTTP \(code)."
            case .decoding:
                return "Could not parse the usage response."
            }
        }
    }

    public func fetch() async throws -> UsageReport {
        let creds = try await credentials.credentials()
        if creds.isExpired {
            await credentials.invalidate()
            throw FetchError.tokenExpired
        }

        var request = URLRequest(url: URL(string: "https://api.anthropic.com/api/oauth/usage")!)
        request.httpMethod = "GET"
        request.setValue("Bearer \(creds.accessToken)", forHTTPHeaderField: "Authorization")
        request.setValue("oauth-2025-04-20", forHTTPHeaderField: "anthropic-beta")
        request.setValue(userAgent, forHTTPHeaderField: "User-Agent")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")

        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else { throw FetchError.decoding }

        switch http.statusCode {
        case 200: break
        case 401: throw FetchError.unauthorized
        case 429: throw FetchError.rateLimited
        default: throw FetchError.server(http.statusCode)
        }

        guard let payload = try? JSONDecoder().decode(UsagePayload.self, from: data) else {
            throw FetchError.decoding
        }

        let windows: [UsageWindow] = [
            payload.five_hour?.asWindow(title: "5-Hour"),
            payload.seven_day?.asWindow(title: "Weekly"),
            payload.extra_usage?.asWindow(title: "Extra usage"),
        ].compactMap { $0 }

        return UsageReport(body: .windows(windows), sourceLabel: sourceLabel)
    }
}

/// Mirrors the endpoint's JSON. Unknown fields are ignored.
private struct UsagePayload: Decodable {
    let five_hour: Window?
    let seven_day: Window?
    let extra_usage: Extra?

    struct Window: Decodable {
        let utilization: Double?      // 0...100
        let resets_at: String?

        func asWindow(title: String) -> UsageWindow {
            UsageWindow(
                utilization: (utilization ?? 0) / 100,
                resetsAt: resets_at.flatMap(UsagePayload.parseDate),
                title: title
            )
        }
    }

    struct Extra: Decodable {
        let is_enabled: Bool?
        let utilization: Double?      // 0...100

        /// Always returns a window — when disabled, a drained "Off" bar so the
        /// UI conveys "feature inactive" instead of pretending the slot is full.
        func asWindow(title: String) -> UsageWindow {
            if is_enabled == true {
                return UsageWindow(utilization: (utilization ?? 0) / 100, title: title)
            }
            return UsageWindow(utilization: 1.0, title: title, trailing: "Off")
        }
    }

    // The endpoint returns fractional seconds (e.g. "...:00.795109+00:00"),
    // which the default ISO8601 formatter rejects — try that form first.
    private static let isoFractional: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()
    private static let isoPlain: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime]
        return f
    }()

    static func parseDate(_ s: String) -> Date? {
        isoFractional.date(from: s) ?? isoPlain.date(from: s)
    }
}
