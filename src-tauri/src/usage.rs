//! Fetches live subscription quota from Anthropic's undocumented OAuth usage
//! endpoint — the same data Claude Code's `/usage` view shows. Port of the
//! Swift `OAuthUsageDataSource`.
//!
//! ```text
//! GET https://api.anthropic.com/api/oauth/usage
//! Authorization: Bearer <oauth access token>
//! anthropic-beta: oauth-2025-04-20
//! User-Agent: claude-code/<version>
//! ```
//!
//! The `User-Agent` is required; without it the request lands in an
//! aggressively rate-limited bucket and returns 429s.

use crate::credentials::CredentialCache;
use serde::{Deserialize, Serialize};

/// Pinned to a recent Claude Code release — the endpoint keys rate limits off
/// this UA. Bump occasionally if Anthropic tightens the allowed range.
const CLAUDE_CODE_VERSION: &str = "2.1.152";
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// One quota window (5-hour / weekly / extra), shaped for the frontend.
#[derive(Serialize, Clone, Debug)]
pub struct UsageWindow {
    /// Fraction consumed, 0..1.
    pub utilization: f64,
    /// Fraction remaining — what the HP-style draining bar fills to.
    pub remaining: f64,
    /// RFC3339 reset timestamp, if known.
    pub resets_at: Option<String>,
    pub title: String,
    /// Provider-reported window length. The burn estimator uses this to choose
    /// an appropriate lookback instead of assuming every limit is five hours.
    pub window_minutes: Option<i64>,
    /// Optional trailing badge (e.g. "Off" for disabled extra usage).
    pub trailing: Option<String>,
    /// Projected seconds until this window hits its limit at the recent burn
    /// rate, set by `ambient::annotate` only when that lands *before* the reset
    /// (i.e. a real "you'll run out" warning). `None` otherwise. See `burn`.
    pub eta_secs: Option<i64>,
    /// This machine's estimated share of the window (0..1), set by
    /// `share::annotate`. `None` until the fit is confident. See `share`.
    pub machine_share: Option<f64>,
    /// Other devices' estimated share of the window (0..1) = utilization −
    /// machine_share. `None` when machine_share is.
    pub others_share: Option<f64>,
    /// Fit confidence 0..1 for the share split; the UI hides the split below a
    /// threshold and hedges ("≈") in the mid range.
    pub share_confidence: Option<f64>,
    /// Estimated window budget `Q` in local $ (the cost that ≈ 100% of the
    /// window). Diagnostic / tooltip only.
    pub window_budget: Option<f64>,
}

#[derive(Serialize, Clone, Debug)]
pub struct UsageDetail {
    pub label: String,
    pub value: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct UsageReport {
    pub windows: Vec<UsageWindow>,
    pub source_label: String,
    pub details: Vec<UsageDetail>,
}

#[derive(Debug)]
pub enum FetchError {
    TokenExpired,
    Unauthorized,
    RateLimited,
    Server(u16),
    Network(String),
    Decoding,
    Credentials(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::TokenExpired | FetchError::Unauthorized => {
                write!(f, "Login expired. Open Claude Code to refresh, then retry.")
            }
            FetchError::RateLimited => write!(f, "Rate limited by Anthropic. Try again shortly."),
            FetchError::Server(code) => write!(f, "Usage endpoint returned HTTP {code}."),
            FetchError::Network(e) => write!(f, "Network error: {e}"),
            FetchError::Decoding => write!(f, "Could not parse the usage response."),
            FetchError::Credentials(e) => write!(f, "{e}"),
        }
    }
}

pub async fn fetch(creds: &CredentialCache) -> Result<UsageReport, FetchError> {
    let credentials = creds
        .get()
        .map_err(|e| FetchError::Credentials(e.to_string()))?;

    if credentials.is_expired() {
        // Note we do *not* schedule a re-read: storage still holds this same
        // dead token until Claude Code refreshes it. See `credentials`.
        creds.mark_rejected();
        return Err(FetchError::TokenExpired);
    }

    let mut client = build_client(None)?;
    let first = fetch_payload(&client, &credentials).await;
    let response = match first {
        // Finder-launched macOS apps do not inherit proxy variables from the
        // login shell. If direct access is rejected, mirror the shell's HTTPS
        // proxy for this client and retry with the already-read token. This
        // neither changes system proxy settings nor touches credential storage.
        Err(error) if should_retry_same_token(&error) => {
            if let Some(proxy_url) = crate::network::login_shell_proxy() {
                client = build_client(Some(&proxy_url))?;
            } else {
                // Also cover a brief auth propagation delay when no proxy is
                // configured. The retry is the same read-only GET.
                tokio::time::sleep(std::time::Duration::from_millis(750)).await;
            }
            fetch_payload(&client, &credentials).await
        }
        other => other,
    };
    let payload = match response {
        Ok(payload) => payload,
        Err(error) if is_credential_rejection(&error) => {
            // This changes HPBar's in-memory cache only. `get()` consults the
            // attribute-only Keychain fingerprint and reads data only if Claude
            // Code has already rotated the item; HPBar never writes or refreshes
            // credentials itself.
            creds.mark_rejected();
            let replacement = match creds.get() {
                Ok(fresh) if fresh.access_token != credentials.access_token => fresh,
                Ok(_) | Err(_) => return Err(error),
            };
            match fetch_payload(&client, &replacement).await {
                Ok(payload) => payload,
                Err(retry_error) => {
                    if is_credential_rejection(&retry_error) {
                        creds.mark_rejected();
                    }
                    return Err(retry_error);
                }
            }
        }
        Err(error) => return Err(error),
    };

    Ok(UsageReport {
        windows: payload.into_windows(),
        source_label: "Live quota".into(),
        details: Vec::new(),
    })
}

fn build_client(proxy_url: Option<&str>) -> Result<reqwest::Client, FetchError> {
    crate::network::client(proxy_url, std::time::Duration::from_secs(20))
        .map_err(|e| FetchError::Network(e.to_string()))
}

async fn fetch_payload(
    client: &reqwest::Client,
    credentials: &crate::credentials::ClaudeCredentials,
) -> Result<Payload, FetchError> {
    let resp = client
        .get(USAGE_URL)
        .header(
            "Authorization",
            format!("Bearer {}", credentials.access_token),
        )
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", format!("claude-code/{CLAUDE_CODE_VERSION}"))
        .header("Content-Type", "application/json")
        .send()
        .await
        .map_err(|e| FetchError::Network(e.to_string()))?;

    match resp.status().as_u16() {
        200 => {}
        401 => return Err(FetchError::Unauthorized),
        403 => {
            // Anthropic sometimes reports an expired/revoked OAuth token as
            // 403 rather than 401. Inspect only the small error response; never
            // log it or the token. Organization-policy 403s stay ordinary
            // server errors and do not invalidate the in-memory credential.
            let challenge = resp
                .headers()
                .get(reqwest::header::WWW_AUTHENTICATE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_string();
            let body = resp.text().await.unwrap_or_default();
            if forbidden_is_oauth_rejection(&challenge, &body) {
                return Err(FetchError::Unauthorized);
            }
            return Err(FetchError::Server(403));
        }
        429 => return Err(FetchError::RateLimited),
        code => return Err(FetchError::Server(code)),
    }

    resp.json().await.map_err(|_| FetchError::Decoding)
}

fn is_credential_rejection(error: &FetchError) -> bool {
    matches!(error, FetchError::Unauthorized | FetchError::TokenExpired)
}

fn should_retry_same_token(error: &FetchError) -> bool {
    matches!(error, FetchError::Unauthorized | FetchError::Server(403))
}

fn forbidden_is_oauth_rejection(challenge: &str, body: &str) -> bool {
    let text = format!("{challenge}\n{body}").to_ascii_lowercase();
    // A new token cannot fix an organization-level ban, so preserve that 403.
    if text.contains("not allowed for this organization")
        || text.contains("organization policy")
    {
        return false;
    }
    let token_rejected = (text.contains("token") || text.contains("bearer"))
        && ["invalid", "expired", "revoked"]
            .iter()
            .any(|state| text.contains(state));
    let scope_rejected = text.contains("scope")
        && ["insufficient", "required", "missing", "lacks"]
            .iter()
            .any(|state| text.contains(state));
    token_rejected
        || scope_rejected
        || text.contains("invalid_token")
        || text.contains("token_expired")
        || text.contains("insufficient_scope")
}

/// Title for a per-model weekly cap, e.g. "Weekly (Fable)".
fn scoped_weekly_title(model: &str) -> String {
    format!("Weekly ({model})")
}

/// True for titles minted by `scoped_weekly_title`. The device-share fit skips
/// these windows: its local-cost series is account-wide, which would skew a
/// per-model window's implied-budget ratio.
pub fn is_model_scoped_title(title: &str) -> bool {
    title.starts_with("Weekly (")
}

/// Mirrors the endpoint's JSON. Unknown fields are ignored.
#[derive(Deserialize)]
struct Payload {
    five_hour: Option<Window>,
    seven_day: Option<Window>,
    extra_usage: Option<Extra>,
    /// Newer, generic list of every limit. The account-wide session/weekly rows
    /// here duplicate `five_hour`/`seven_day`; what's *only* here are the
    /// model-scoped weekly caps (e.g. the Fable weekly limit).
    limits: Option<Vec<Limit>>,
}

impl Payload {
    fn into_windows(self) -> Vec<UsageWindow> {
        let mut windows: Vec<UsageWindow> = [
            self.five_hour.map(|w| w.into_window("5-Hour")),
            self.seven_day.map(|w| w.into_window("Weekly")),
        ]
        .into_iter()
        .flatten()
        .collect();

        // Per-model weekly caps from `limits`: only entries scoped to a model —
        // the unscoped session/weekly rows are already covered above, and a
        // hypothetical non-weekly scoped limit shouldn't get a "Weekly" title.
        for l in self.limits.into_iter().flatten() {
            if l.group.as_deref() != Some("weekly") {
                continue;
            }
            let Some(name) = l
                .scope
                .as_ref()
                .and_then(|s| s.model.as_ref())
                .and_then(|m| m.display_name.clone())
            else {
                continue;
            };
            let util = clamp01(l.percent.unwrap_or(0.0) / 100.0);
            windows.push(UsageWindow {
                utilization: util,
                remaining: clamp01(1.0 - util),
                resets_at: l.resets_at,
                title: scoped_weekly_title(&name),
                window_minutes: Some(10_080),
                trailing: None,
                eta_secs: None,
                machine_share: None,
                others_share: None,
                share_confidence: None,
                window_budget: None,
            });
        }

        if let Some(w) = self.extra_usage.and_then(|e| e.into_window("Extra usage")) {
            windows.push(w);
        }
        windows
    }
}

/// One row of the `limits` array.
#[derive(Deserialize)]
struct Limit {
    group: Option<String>,
    percent: Option<f64>, // 0..100
    resets_at: Option<String>,
    scope: Option<LimitScope>,
}

#[derive(Deserialize)]
struct LimitScope {
    model: Option<LimitScopeModel>,
}

#[derive(Deserialize)]
struct LimitScopeModel {
    display_name: Option<String>,
}

#[derive(Deserialize)]
struct Window {
    utilization: Option<f64>, // 0..100
    resets_at: Option<String>,
}

impl Window {
    fn into_window(self, title: &str) -> UsageWindow {
        let util = clamp01(self.utilization.unwrap_or(0.0) / 100.0);
        UsageWindow {
            utilization: util,
            remaining: clamp01(1.0 - util),
            resets_at: self.resets_at,
            title: title.into(),
            window_minutes: match title {
                "5-Hour" => Some(300),
                "Weekly" => Some(10_080),
                _ => None,
            },
            trailing: None,
            eta_secs: None,
            machine_share: None,
            others_share: None,
            share_confidence: None,
            window_budget: None,
        }
    }
}

#[derive(Deserialize)]
struct Extra {
    is_enabled: Option<bool>,
    utilization: Option<f64>, // 0..100
}

impl Extra {
    /// `None` when the feature is disabled — the bar is hidden entirely rather
    /// than shown as a drained "Off" row (visual clutter now that per-model
    /// weekly caps compete for the same vertical space).
    fn into_window(self, title: &str) -> Option<UsageWindow> {
        if self.is_enabled != Some(true) {
            return None;
        }
        let util = clamp01(self.utilization.unwrap_or(0.0) / 100.0);
        Some(UsageWindow {
            utilization: util,
            remaining: clamp01(1.0 - util),
            resets_at: None,
            title: title.into(),
            window_minutes: None,
            trailing: None,
            eta_secs: None,
            machine_share: None,
            others_share: None,
            share_confidence: None,
            window_budget: None,
        })
    }
}

fn clamp01(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Vec<UsageWindow> {
        serde_json::from_str::<Payload>(json)
            .expect("payload parses")
            .into_windows()
    }

    #[test]
    fn oauth_403_rejections_are_distinguished_from_org_policy() {
        assert!(forbidden_is_oauth_rejection(
            r#"Bearer error="invalid_token""#,
            ""
        ));
        assert!(forbidden_is_oauth_rejection(
            "",
            r#"{"error":{"message":"Invalid OAuth token"}}"#,
        ));
        assert!(forbidden_is_oauth_rejection(
            "",
            r#"{"error":{"message":"OAuth token expired"}}"#,
        ));
        assert!(forbidden_is_oauth_rejection(
            "",
            r#"{"error":{"type":"permission_error","message":"OAuth token is missing required scope user:profile"}}"#,
        ));
        assert!(!forbidden_is_oauth_rejection(
            "",
            r#"{"error":{"type":"authentication_error","message":"OAuth authentication is currently not allowed for this organization"}}"#,
        ));
        assert!(!forbidden_is_oauth_rejection(
            "",
            r#"{"error":{"type":"permission_error","message":"Usage data is disabled by an administrator"}}"#,
        ));
        assert!(should_retry_same_token(&FetchError::Unauthorized));
        assert!(should_retry_same_token(&FetchError::Server(403)));
        assert!(!should_retry_same_token(&FetchError::RateLimited));
    }

    #[test]
    fn scoped_weekly_limit_becomes_its_own_bar() {
        // Trimmed from a real response: the Fable weekly cap arrives only as a
        // model-scoped row in `limits`; the unscoped session/weekly rows there
        // must not duplicate the five_hour/seven_day bars.
        let windows = parse(
            r#"{
                "five_hour": { "utilization": 24.0, "resets_at": "2026-07-27T06:39:59+00:00" },
                "seven_day": { "utilization": 24.0, "resets_at": "2026-07-29T14:59:59+00:00" },
                "limits": [
                    { "kind": "session", "group": "session", "percent": 24, "resets_at": "2026-07-27T06:39:59+00:00", "scope": null },
                    { "kind": "weekly_all", "group": "weekly", "percent": 24, "resets_at": "2026-07-29T14:59:59+00:00", "scope": null },
                    { "kind": "weekly_scoped", "group": "weekly", "percent": 3, "resets_at": "2026-07-29T14:59:59+00:00",
                      "scope": { "model": { "id": null, "display_name": "Fable" }, "surface": null } }
                ],
                "extra_usage": { "is_enabled": false, "utilization": null }
            }"#,
        );
        let titles: Vec<&str> = windows.iter().map(|w| w.title.as_str()).collect();
        assert_eq!(titles, vec!["5-Hour", "Weekly", "Weekly (Fable)"]);
        let fable = &windows[2];
        assert!((fable.utilization - 0.03).abs() < 1e-9);
        assert!((fable.remaining - 0.97).abs() < 1e-9);
        assert_eq!(
            fable.resets_at.as_deref(),
            Some("2026-07-29T14:59:59+00:00")
        );
        assert!(is_model_scoped_title(&fable.title));
        assert!(!is_model_scoped_title("Weekly"));
    }

    #[test]
    fn disabled_extra_usage_is_hidden() {
        let windows = parse(
            r#"{
                "five_hour": { "utilization": 10.0, "resets_at": null },
                "extra_usage": { "is_enabled": false, "utilization": null }
            }"#,
        );
        assert!(windows.iter().all(|w| w.title != "Extra usage"));
    }

    #[test]
    fn enabled_extra_usage_still_shows() {
        let windows = parse(
            r#"{
                "extra_usage": { "is_enabled": true, "utilization": 40.0 }
            }"#,
        );
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].title, "Extra usage");
        assert!((windows[0].remaining - 0.6).abs() < 1e-9);
        assert_eq!(windows[0].trailing, None);
    }

    #[test]
    fn payload_without_limits_still_parses() {
        // Older response shape (no `limits` key at all).
        let windows = parse(
            r#"{
                "five_hour": { "utilization": 50.0, "resets_at": null },
                "seven_day": { "utilization": 20.0, "resets_at": null }
            }"#,
        );
        let titles: Vec<&str> = windows.iter().map(|w| w.title.as_str()).collect();
        assert_eq!(titles, vec!["5-Hour", "Weekly"]);
    }
}
