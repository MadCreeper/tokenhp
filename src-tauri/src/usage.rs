//! Fetches live subscription quota from Anthropic's undocumented OAuth usage
//! endpoint — the same data Claude Code's `/usage` view shows. Port of the
//! Swift `OAuthUsageDataSource`.
//!
//!     GET https://api.anthropic.com/api/oauth/usage
//!     Authorization: Bearer <oauth access token>
//!     anthropic-beta: oauth-2025-04-20
//!     User-Agent: claude-code/<version>
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
    /// Optional trailing badge (e.g. "Off" for disabled extra usage).
    pub trailing: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct UsageReport {
    pub windows: Vec<UsageWindow>,
    pub source_label: String,
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
            FetchError::TokenExpired | FetchError::Unauthorized => write!(
                f,
                "Login expired. Open Claude Code to refresh, then retry."
            ),
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
        creds.invalidate();
        return Err(FetchError::TokenExpired);
    }

    let client = reqwest::Client::new();
    let resp = client
        .get(USAGE_URL)
        .header("Authorization", format!("Bearer {}", credentials.access_token))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", format!("claude-code/{CLAUDE_CODE_VERSION}"))
        .header("Content-Type", "application/json")
        .send()
        .await
        .map_err(|e| FetchError::Network(e.to_string()))?;

    match resp.status().as_u16() {
        200 => {}
        401 => {
            // The cached token was rejected — almost always because Claude Code
            // rotated it out from under us. Drop our cache so the next fetch
            // re-reads storage (picking up the new token) instead of looping.
            creds.invalidate();
            return Err(FetchError::Unauthorized);
        }
        429 => return Err(FetchError::RateLimited),
        code => return Err(FetchError::Server(code)),
    }

    let payload: Payload = resp.json().await.map_err(|_| FetchError::Decoding)?;

    let windows = [
        payload.five_hour.map(|w| w.into_window("5-Hour")),
        payload.seven_day.map(|w| w.into_window("Weekly")),
        payload.extra_usage.map(|e| e.into_window("Extra usage")),
    ]
    .into_iter()
    .flatten()
    .collect();

    Ok(UsageReport {
        windows,
        source_label: "Live quota".into(),
    })
}

/// Mirrors the endpoint's JSON. Unknown fields are ignored.
#[derive(Deserialize)]
struct Payload {
    five_hour: Option<Window>,
    seven_day: Option<Window>,
    extra_usage: Option<Extra>,
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
            trailing: None,
        }
    }
}

#[derive(Deserialize)]
struct Extra {
    is_enabled: Option<bool>,
    utilization: Option<f64>, // 0..100
}

impl Extra {
    /// Always returns a window — when disabled, a drained "Off" bar so the UI
    /// conveys "feature inactive" instead of pretending the slot is full.
    fn into_window(self, title: &str) -> UsageWindow {
        if self.is_enabled == Some(true) {
            let util = clamp01(self.utilization.unwrap_or(0.0) / 100.0);
            UsageWindow {
                utilization: util,
                remaining: clamp01(1.0 - util),
                resets_at: None,
                title: title.into(),
                trailing: None,
            }
        } else {
            UsageWindow {
                utilization: 1.0,
                remaining: 0.0,
                resets_at: None,
                title: title.into(),
                trailing: Some("Off".into()),
            }
        }
    }
}

fn clamp01(v: f64) -> f64 {
    v.max(0.0).min(1.0)
}
