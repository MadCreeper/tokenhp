//! Best-effort account identity for the footer: which login this machine is
//! using and what plan it's on. Two local sources, both shared by Claude Code:
//!   - email: `~/.claude.json` → `oauthAccount.emailAddress`
//!   - plan:  the OAuth credential's `subscriptionType` + `rateLimitTier`
//!            (file on Linux/Windows, Keychain on macOS — see `credentials`).
//!
//! Everything is optional; we render whatever we can read and stay silent on
//! the rest rather than erroring.

use crate::credentials::CredentialCache;
use serde::Serialize;

#[derive(Serialize, Clone, Default, Debug)]
pub struct AccountInfo {
    /// Login email, e.g. "name@example.com".
    pub email: Option<String>,
    /// Human plan label, e.g. "Max 20×" / "Pro".
    pub plan: Option<String>,
}

pub fn fetch(creds: &CredentialCache) -> AccountInfo {
    let plan = creds
        .get()
        .ok()
        .and_then(|c| plan_label(c.subscription_type.as_deref(), c.rate_limit_tier.as_deref()));

    AccountInfo {
        email: read_email(),
        plan,
    }
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
fn decode_b64url(s: &str) -> Option<Vec<u8>> {
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

/// Pull the login email out of `~/.claude.json` (or `$CLAUDE_CONFIG_DIR`'s copy
/// — see [`crate::credentials::claude_json_path`]). Same shape on every OS.
pub fn read_email() -> Option<String> {
    let path = crate::credentials::claude_json_path()?;
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get("oauthAccount")?
        .get("emailAddress")?
        .as_str()
        .map(str::to_string)
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
