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
        // Note we do *not* schedule a re-read: storage still holds this same
        // dead token until Claude Code refreshes it. See `credentials`.
        creds.mark_rejected();
        return Err(FetchError::TokenExpired);
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| FetchError::Network(e.to_string()))?;
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
            // rotated it out from under us. Mark it dead; the next fetch picks
            // up the replacement as soon as Claude Code writes one, without
            // re-reading (and re-prompting for) storage in the meantime.
            creds.mark_rejected();
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
                eta_secs: None,
                machine_share: None,
                others_share: None,
                share_confidence: None,
                window_budget: None,
            }
        } else {
            UsageWindow {
                utilization: 1.0,
                remaining: 0.0,
                resets_at: None,
                title: title.into(),
                trailing: Some("Off".into()),
                eta_secs: None,
                machine_share: None,
                others_share: None,
                share_confidence: None,
                window_budget: None,
            }
        }
    }
}

fn clamp01(v: f64) -> f64 {
    v.max(0.0).min(1.0)
}
