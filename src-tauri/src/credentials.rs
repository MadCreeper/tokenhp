//! Reads the Claude Code OAuth token from local storage.
//!
//! Port of the Swift `ClaudeCredentials` + `CredentialProvider`. The storage
//! location differs per OS:
//!   - macOS:        Keychain generic-password item `Claude Code-credentials`
//!                   (account = the OS username).
//!   - Linux / Win:  plaintext file `~/.claude/.credentials.json`.
//!
//! All three hold the same JSON shape:
//!   { "claudeAiOauth": { "accessToken": "...", "expiresAt": <epoch_ms> } }

use serde::Deserialize;
use std::sync::Mutex;

/// Re-read the token once it's within this many seconds of expiry.
const REFRESH_MARGIN_SECS: f64 = 120.0;

#[derive(Clone, Debug)]
pub struct ClaudeCredentials {
    pub access_token: String,
    /// Expiry as Unix epoch seconds, if known.
    pub expires_at: Option<f64>,
    /// Plan tier, e.g. "max" / "pro" / "free".
    pub subscription_type: Option<String>,
    /// Rate-limit tier, e.g. "default_claude_max_20x" — encodes the 5×/20× multiplier.
    pub rate_limit_tier: Option<String>,
}

impl ClaudeCredentials {
    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(exp) => exp <= now_secs(),
            None => false,
        }
    }

    /// Within `REFRESH_MARGIN_SECS` of expiry — time to re-read.
    fn is_stale(&self) -> bool {
        match self.expires_at {
            Some(exp) => exp - now_secs() <= REFRESH_MARGIN_SECS,
            None => false,
        }
    }
}

#[derive(Debug)]
pub enum CredError {
    NotFound,
    AccessDenied(String),
    Malformed,
}

impl std::fmt::Display for CredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CredError::NotFound => write!(
                f,
                "No Claude Code login found. Sign in with the Claude Code CLI first."
            ),
            CredError::AccessDenied(s) => {
                write!(f, "Could not read Claude Code credentials: {s}")
            }
            CredError::Malformed => write!(
                f,
                "Stored Claude Code credentials were not in the expected format."
            ),
        }
    }
}

/// Caches credentials in memory so storage is read only when the cached token
/// is missing or near expiry — not on every poll. Mirrors the Swift
/// `CredentialProvider` actor (and, on macOS, keeps Keychain prompts rare).
#[derive(Default)]
pub struct CredentialCache {
    cached: Mutex<Option<ClaudeCredentials>>,
}

impl CredentialCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// A usable credential. Reads storage only when the cache is empty or the
    /// cached token is within the refresh margin of expiry.
    pub fn get(&self) -> Result<ClaudeCredentials, CredError> {
        {
            let guard = self.cached.lock().unwrap();
            if let Some(c) = guard.as_ref() {
                if !c.is_stale() {
                    return Ok(c.clone());
                }
            }
        }
        let fresh = load_from_storage()?;
        *self.cached.lock().unwrap() = Some(fresh.clone());
        Ok(fresh)
    }

    /// Drop the cache so the next `get()` re-reads storage (e.g. after a 401).
    pub fn invalidate(&self) {
        *self.cached.lock().unwrap() = None;
    }
}

fn now_secs() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn load_from_storage() -> Result<ClaudeCredentials, CredError> {
    let raw = read_raw()?;
    parse(&raw)
}

#[cfg(target_os = "macos")]
fn read_raw() -> Result<String, CredError> {
    // The item is stored under account = the OS username (verified empirically).
    let user = std::env::var("USER").unwrap_or_default();
    let entry = keyring::Entry::new("Claude Code-credentials", &user)
        .map_err(|e| CredError::AccessDenied(e.to_string()))?;
    match entry.get_password() {
        Ok(s) => Ok(s),
        Err(keyring::Error::NoEntry) => Err(CredError::NotFound),
        Err(e) => Err(CredError::AccessDenied(e.to_string())),
    }
}

#[cfg(not(target_os = "macos"))]
fn read_raw() -> Result<String, CredError> {
    let path = dirs::home_dir()
        .ok_or_else(|| CredError::AccessDenied("no home directory".into()))?
        .join(".claude")
        .join(".credentials.json");
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(CredError::NotFound),
        Err(e) => Err(CredError::AccessDenied(e.to_string())),
    }
}

fn parse(raw: &str) -> Result<ClaudeCredentials, CredError> {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(rename = "claudeAiOauth")]
        oauth: OAuth,
    }
    #[derive(Deserialize)]
    struct OAuth {
        #[serde(rename = "accessToken")]
        access_token: String,
        // Stored in epoch milliseconds.
        #[serde(rename = "expiresAt")]
        expires_at: Option<f64>,
        #[serde(rename = "subscriptionType")]
        subscription_type: Option<String>,
        #[serde(rename = "rateLimitTier")]
        rate_limit_tier: Option<String>,
    }

    let env: Envelope = serde_json::from_str(raw).map_err(|_| CredError::Malformed)?;
    Ok(ClaudeCredentials {
        access_token: env.oauth.access_token,
        expires_at: env.oauth.expires_at.map(|ms| ms / 1000.0),
        subscription_type: env.oauth.subscription_type,
        rate_limit_tier: env.oauth.rate_limit_tier,
    })
}
