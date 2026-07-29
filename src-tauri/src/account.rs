//! Best-effort account identity for the footer: which login this machine is
//! using and what plan it's on. Local sources, all shared with Claude Code:
//!   - email + plan: `~/.claude.json` → `oauthAccount` — Claude Code's cached
//!     copy of the live profile, rewritten roughly every time CC launches. The
//!     freshest source that needs no auth at all (plain file read).
//!   - plan fallback: the OAuth credential's `subscriptionType` + `rateLimitTier`
//!     (file on Linux/Windows, Keychain on macOS — see `credentials`). Stamped
//!     at token issuance, so it can lag a plan change indefinitely — used only
//!     when the profile block is missing.
//!
//! Everything is optional; we render whatever we can read and stay silent on
//! the rest rather than erroring.

use crate::credentials::CredentialCache;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[derive(Serialize, Clone, Default, Debug)]
pub struct AccountInfo {
    /// Login email, e.g. "name@example.com".
    pub email: Option<String>,
    /// Human plan label, e.g. "Max 20×" / "Pro".
    pub plan: Option<String>,
}

/// Stable, non-secret identity used to group usage from the same subscription
/// account across machines. The raw provider UUID never leaves this module.
#[derive(Clone, Debug)]
pub struct UsageIdentity {
    pub account_key: String,
    pub billing_key: String,
    pub label: String,
}

/// Result of mapping a historical usage event to an account observation epoch.
#[derive(Clone, Debug)]
pub struct UsageAttribution {
    pub account_key: String,
    pub billing_key: String,
    pub account_label: String,
    /// "exact" means HPBar was running and continuously observed this account;
    /// "unknown" is used for history/gaps where the source log has no identity.
    pub status: String,
}

impl UsageAttribution {
    pub fn unknown() -> Self {
        UsageAttribution {
            account_key: "unknown".into(),
            billing_key: "unknown".into(),
            account_label: "Unknown account".into(),
            status: "unknown".into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct AccountEpoch {
    provider: String,
    account_key: String,
    billing_key: String,
    label: String,
    starts_at: i64,
    last_seen_at: i64,
    ends_at: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct AccountHistory {
    version: u32,
    epochs: Vec<AccountEpoch>,
}

impl Default for AccountHistory {
    fn default() -> Self {
        AccountHistory {
            version: 1,
            epochs: Vec::new(),
        }
    }
}

static HISTORY_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static OBSERVER_STARTED: OnceLock<()> = OnceLock::new();

pub fn fetch(creds: &CredentialCache) -> AccountInfo {
    let acct = read_oauth_account();
    let email = acct
        .as_ref()
        .and_then(|a| a.get("emailAddress"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let plan = acct.as_ref().and_then(plan_from_oauth_account).or_else(|| {
        // Never touches storage while the cached token is healthy, so this
        // stays safe to call on every poll.
        creds
            .get()
            .ok()
            .and_then(|c| plan_label(c.subscription_type.as_deref(), c.rate_limit_tier.as_deref()))
    });

    AccountInfo { email, plan }
}

pub fn claude_identity() -> Option<UsageIdentity> {
    let acct = read_oauth_account()?;
    let email = acct
        .get("emailAddress")
        .and_then(|v| v.as_str())
        .unwrap_or("Claude account")
        .to_string();
    let raw_account = acct
        .get("accountUuid")
        .and_then(|v| v.as_str())
        .or_else(|| acct.get("organizationUuid").and_then(|v| v.as_str()))
        .or_else(|| acct.get("emailAddress").and_then(|v| v.as_str()))?;
    let raw_billing = acct
        .get("organizationUuid")
        .and_then(|v| v.as_str())
        .unwrap_or(raw_account);
    Some(UsageIdentity {
        account_key: stable_key("claude-account", raw_account),
        billing_key: stable_key("claude-billing", raw_billing),
        label: email,
    })
}

/// Codex (ChatGPT) identity from `~/.codex/auth.json`'s `id_token` JWT claims.
/// We only read claims for display — no signature verification.
pub fn fetch_codex() -> AccountInfo {
    let claims = read_codex_id_claims();
    let email = claims
        .as_ref()
        .and_then(|c| c.get("email"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let plan = claims
        .as_ref()
        .and_then(|c| c.get("https://api.openai.com/auth"))
        .and_then(|a| a.get("chatgpt_plan_type"))
        .and_then(|v| v.as_str())
        .map(capitalize);
    AccountInfo { email, plan }
}

pub fn codex_identity() -> Option<UsageIdentity> {
    let claims = read_codex_id_claims()?;
    let auth = claims.get("https://api.openai.com/auth");
    let email = claims
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("Codex account")
        .to_string();
    let raw_account = auth
        .and_then(|a| a.get("chatgpt_account_id"))
        .and_then(|v| v.as_str())
        .or_else(|| claims.get("sub").and_then(|v| v.as_str()))
        .or_else(|| claims.get("email").and_then(|v| v.as_str()))?;
    // Personal plans bill at account scope. Workspace-aware clients can later
    // replace this with a selected organization id without changing the schema.
    Some(UsageIdentity {
        account_key: stable_key("codex-account", raw_account),
        billing_key: stable_key("codex-billing", raw_account),
        label: email,
    })
}

fn stable_key(namespace: &str, raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(namespace.as_bytes());
    hasher.update(b":");
    hasher.update(raw.trim().to_lowercase().as_bytes());
    let digest = hasher.finalize();
    format!("{namespace}-{}", hex_prefix(&digest, 12))
}

fn hex_prefix(bytes: &[u8], take: usize) -> String {
    let mut out = String::with_capacity(take * 2);
    for byte in bytes.iter().take(take) {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

/// Record the accounts HPBar can currently observe. Call once at startup and
/// periodically thereafter. A process restart deliberately creates a new epoch,
/// leaving the offline gap unattributed instead of guessing that no switch
/// happened while HPBar was closed.
pub fn start_observer_run() {
    if OBSERVER_STARTED.set(()).is_err() {
        observe_current_accounts();
        return;
    }
    with_history(|history, now| {
        for epoch in history.epochs.iter_mut().filter(|e| e.ends_at.is_none()) {
            epoch.ends_at = Some(epoch.last_seen_at);
        }
        observe_identity(history, "claude", claude_identity(), now);
        observe_identity(history, "codex", codex_identity(), now);
    });
}

pub fn observe_current_accounts() {
    with_history(|history, now| {
        observe_identity(history, "claude", claude_identity(), now);
        observe_identity(history, "codex", codex_identity(), now);
    });
}

pub fn attribution(provider: &str, timestamp: i64) -> UsageAttribution {
    let _guard = HISTORY_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let history = load_history();
    history
        .epochs
        .iter()
        .rev()
        .find(|e| {
            e.provider == provider
                && timestamp >= e.starts_at
                && e.ends_at.map_or(true, |end| timestamp <= end)
        })
        .map(|e| UsageAttribution {
            account_key: e.account_key.clone(),
            billing_key: e.billing_key.clone(),
            account_label: e.label.clone(),
            status: "exact".into(),
        })
        .unwrap_or_else(UsageAttribution::unknown)
}

fn observe_identity(
    history: &mut AccountHistory,
    provider: &str,
    identity: Option<UsageIdentity>,
    now: i64,
) {
    let open = history
        .epochs
        .iter()
        .rposition(|e| e.provider == provider && e.ends_at.is_none());
    match (open, identity) {
        (Some(i), Some(identity)) if history.epochs[i].account_key == identity.account_key => {
            let epoch = &mut history.epochs[i];
            epoch.last_seen_at = now;
            epoch.label = identity.label;
            epoch.billing_key = identity.billing_key;
        }
        (Some(i), next) => {
            // Anything since the last observation is ambiguous: close at the
            // last known-good instant rather than assigning the gap to either
            // account.
            history.epochs[i].ends_at = Some(history.epochs[i].last_seen_at);
            if let Some(identity) = next {
                history.epochs.push(AccountEpoch {
                    provider: provider.into(),
                    account_key: identity.account_key,
                    billing_key: identity.billing_key,
                    label: identity.label,
                    starts_at: now,
                    last_seen_at: now,
                    ends_at: None,
                });
            }
        }
        (None, Some(identity)) => history.epochs.push(AccountEpoch {
            provider: provider.into(),
            account_key: identity.account_key,
            billing_key: identity.billing_key,
            label: identity.label,
            starts_at: now,
            last_seen_at: now,
            ends_at: None,
        }),
        (None, None) => {}
    }
    // Bound a corrupt/ancient file without losing useful billing history.
    if history.epochs.len() > 2_000 {
        history.epochs.drain(..history.epochs.len() - 2_000);
    }
}

fn with_history(f: impl FnOnce(&mut AccountHistory, i64)) {
    let _guard = HISTORY_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut history = load_history();
    f(&mut history, chrono::Utc::now().timestamp());
    save_history(&history);
}

fn history_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("HPBar").join("account-history.json"))
}

fn load_history() -> AccountHistory {
    history_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_history(history: &AccountHistory) {
    let Some(path) = history_path() else {
        return;
    };
    let Some(dir) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let Ok(body) = serde_json::to_vec_pretty(history) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, body).is_ok() {
        restrict_file_permissions(&tmp);
        // Windows rename does not replace an existing destination. The small
        // remove/rename gap is preferable to silently freezing attribution
        // after the first observation.
        #[cfg(target_os = "windows")]
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::rename(tmp, path);
    }
}

#[cfg(unix)]
fn restrict_file_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_file_permissions(_path: &std::path::Path) {}

/// Decode the (middle) payload of the stored `id_token` JWT into JSON claims.
fn read_codex_id_claims() -> Option<serde_json::Value> {
    let path = dirs::home_dir()?.join(".codex").join("auth.json");
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let token = v.get("tokens")?.get("id_token")?.as_str()?;
    let payload = token.split('.').nth(1)?;
    let bytes = decode_b64url(payload)?;
    serde_json::from_slice(&bytes).ok()
}

/// Minimal URL-safe base64 decoder (no padding) — enough for a JWT payload.
pub(crate) fn decode_b64url(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut buf, mut bits) = (0u32, 0u32);
    for &c in s.as_bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            b'=' => continue,
            _ => return None,
        } as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

/// The `oauthAccount` block of `~/.claude.json` (or `$CLAUDE_CONFIG_DIR`'s copy
/// — see [`crate::credentials::claude_json_path`]). Same shape on every OS.
fn read_oauth_account() -> Option<serde_json::Value> {
    let path = crate::credentials::claude_json_path()?;
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get("oauthAccount").cloned()
}

/// Pull the login email out of `~/.claude.json`.
pub fn read_email() -> Option<String> {
    read_oauth_account()?
        .get("emailAddress")?
        .as_str()
        .map(str::to_string)
}

/// Plan from the cached profile block: `organizationType` names the plan,
/// `userRateLimitTier` (per-seat, when set) or `organizationRateLimitTier`
/// carries the multiplier. `None` when the block doesn't name a plan — the
/// caller then falls back to the credential's stashed fields.
fn plan_from_oauth_account(acct: &serde_json::Value) -> Option<String> {
    let org_type = acct.get("organizationType")?.as_str()?;
    let base = match org_type {
        "claude_max" => "Max".to_string(),
        "claude_pro" => "Pro".to_string(),
        "claude_free" => "Free".to_string(),
        "claude_team" => "Team".to_string(),
        "claude_enterprise" => "Enterprise".to_string(),
        other => capitalize(other.strip_prefix("claude_").unwrap_or(other)),
    };
    let tier = ["userRateLimitTier", "organizationRateLimitTier"]
        .iter()
        .find_map(|k| acct.get(*k).and_then(|v| v.as_str()));
    match tier.and_then(parse_multiplier) {
        Some(mult) => Some(format!("{base} {mult}×")),
        None => Some(base),
    }
}

/// "max" + "default_claude_max_20x" → "Max 20×"; "pro" → "Pro".
fn plan_label(subscription_type: Option<&str>, rate_limit_tier: Option<&str>) -> Option<String> {
    let sub = subscription_type?;
    let base = match sub.to_lowercase().as_str() {
        "max" => "Max".to_string(),
        "pro" => "Pro".to_string(),
        "free" => "Free".to_string(),
        "team" => "Team".to_string(),
        _ => capitalize(sub),
    };
    match rate_limit_tier.and_then(parse_multiplier) {
        Some(mult) => Some(format!("{base} {mult}×")),
        None => Some(base),
    }
}

/// Extract the "20" from a tier like "default_claude_max_20x".
fn parse_multiplier(tier: &str) -> Option<String> {
    tier.split(['_', '-']).find_map(|part| {
        let num = part.strip_suffix('x')?;
        (!num.is_empty() && num.bytes().all(|b| b.is_ascii_digit())).then(|| num.to_string())
    })
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plan_reads_org_type_and_tier() {
        let acct = json!({
            "organizationType": "claude_max",
            "organizationRateLimitTier": "default_claude_max_20x",
            "userRateLimitTier": null,
        });
        assert_eq!(plan_from_oauth_account(&acct).as_deref(), Some("Max 20×"));
    }

    #[test]
    fn user_tier_beats_org_tier() {
        // Team/Enterprise seats can carry a per-user tier; it's more specific.
        let acct = json!({
            "organizationType": "claude_team",
            "organizationRateLimitTier": "default_claude_max_5x",
            "userRateLimitTier": "default_claude_max_20x",
        });
        assert_eq!(plan_from_oauth_account(&acct).as_deref(), Some("Team 20×"));
    }

    #[test]
    fn plan_without_tier_is_just_the_base() {
        let acct = json!({ "organizationType": "claude_pro" });
        assert_eq!(plan_from_oauth_account(&acct).as_deref(), Some("Pro"));
    }

    #[test]
    fn missing_org_type_defers_to_credential_fallback() {
        assert_eq!(plan_from_oauth_account(&json!({})), None);
    }

    #[test]
    fn unknown_org_type_degrades_gracefully() {
        let acct = json!({ "organizationType": "claude_something_new" });
        assert_eq!(
            plan_from_oauth_account(&acct).as_deref(),
            Some("Something_new")
        );
    }

    #[test]
    fn credential_fallback_labels() {
        assert_eq!(
            plan_label(Some("max"), Some("default_claude_max_20x")).as_deref(),
            Some("Max 20×")
        );
        assert_eq!(plan_label(Some("pro"), None).as_deref(), Some("Pro"));
        assert_eq!(plan_label(None, Some("default_claude_max_20x")), None);
    }

    #[test]
    fn account_keys_are_stable_and_do_not_expose_identity() {
        let key = stable_key("claude-account", "person@example.com");
        assert_eq!(key, stable_key("claude-account", "PERSON@example.com"));
        assert!(key.starts_with("claude-account-"));
        assert!(!key.contains("person"));
        assert!(!key.contains('@'));
    }

    #[test]
    fn account_switch_leaves_ambiguous_gap_unknown() {
        let mut history = AccountHistory::default();
        let first = UsageIdentity {
            account_key: "a".into(),
            billing_key: "ba".into(),
            label: "a@example.com".into(),
        };
        let second = UsageIdentity {
            account_key: "b".into(),
            billing_key: "bb".into(),
            label: "b@example.com".into(),
        };
        observe_identity(&mut history, "claude", Some(first.clone()), 100);
        observe_identity(&mut history, "claude", Some(first), 130);
        observe_identity(&mut history, "claude", Some(second), 160);
        assert_eq!(history.epochs.len(), 2);
        assert_eq!(history.epochs[0].ends_at, Some(130));
        assert_eq!(history.epochs[1].starts_at, 160);
    }
}
