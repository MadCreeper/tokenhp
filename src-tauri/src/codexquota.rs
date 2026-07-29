//! Live Codex (ChatGPT) rate-limit quota from the backend usage endpoint the
//! CLI itself queries:
//!
//! ```text
//! GET https://chatgpt.com/backend-api/wham/usage
//! Authorization: Bearer <access_token from ~/.codex/auth.json>
//! chatgpt-account-id: <tokens.account_id>
//! originator: codex_cli_rs
//! User-Agent: codex_cli_rs/<version>
//! ```
//!
//! Codex ≥ 0.145 only writes a rate-limit snapshot into a session rollout when
//! a real turn completes, so on an idle machine the local scan in `codexstats`
//! can be arbitrarily stale. This module is the primary source; the rollout
//! scan survives as an offline fallback with a freshness guard.
//!
//! Auth is strictly read-only: we use the access token Codex already saved and
//! NEVER run the OAuth refresh flow or write `auth.json` — refresh-token
//! rotation would invalidate the CLI's stored login (same principle as
//! `credentials` for Claude Code). An expired token is an error telling the
//! user to open Codex, which refreshes it itself.

use crate::usage::{UsageDetail, UsageReport, UsageWindow};
use serde::Deserialize;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
/// The endpoint keys behavior off the CLI's originator/UA pair; pin a recent
/// release and bump occasionally.
const CODEX_CLI_VERSION: &str = "0.145.0";

pub async fn fetch() -> Result<UsageReport, String> {
    let auth = read_auth().ok_or_else(|| {
        "No Codex ChatGPT login found (~/.codex/auth.json). \
         API-key billing has no rate-limit quota."
            .to_string()
    })?;
    if auth.is_expired() {
        return Err("Codex login expired. Open Codex to refresh it, then retry.".into());
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("Network error: {e}"))?;
    let resp = client
        .get(USAGE_URL)
        .header("Authorization", format!("Bearer {}", auth.access_token))
        .header("chatgpt-account-id", &auth.account_id)
        .header("originator", "codex_cli_rs")
        .header("User-Agent", format!("codex_cli_rs/{CODEX_CLI_VERSION}"))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    match resp.status().as_u16() {
        200 => {}
        401 | 403 => {
            return Err("Codex login was rejected. Open Codex to refresh it, then retry.".into())
        }
        429 => return Err("Rate limited by OpenAI. Try again shortly.".into()),
        code => return Err(format!("Codex usage endpoint returned HTTP {code}.")),
    }

    let payload: Payload = resp
        .json()
        .await
        .map_err(|_| "Could not parse the Codex usage response.".to_string())?;
    payload.into_report()
}

struct Auth {
    access_token: String,
    account_id: String,
}

impl Auth {
    /// Expiry from the access token's own `exp` claim — no network, no refresh.
    /// An undecodable token doesn't block the fetch; the server will 401.
    fn is_expired(&self) -> bool {
        let Some(exp) = jwt_claims(&self.access_token)
            .and_then(|c| c.get("exp").and_then(|v| v.as_i64()))
        else {
            return false;
        };
        exp <= chrono::Utc::now().timestamp()
    }
}

/// Tokens Codex saved at login. Absent for API-key-only auth.
fn read_auth() -> Option<Auth> {
    let path = dirs::home_dir()?.join(".codex").join("auth.json");
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let tokens = v.get("tokens")?;
    let access_token = tokens.get("access_token")?.as_str()?.to_string();
    // account_id sits next to the tokens; older files only carry it inside the
    // access token's auth claim.
    let account_id = tokens
        .get("account_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            jwt_claims(&access_token)?
                .get("https://api.openai.com/auth")?
                .get("chatgpt_account_id")?
                .as_str()
                .map(str::to_string)
        })?;
    Some(Auth {
        access_token,
        account_id,
    })
}

/// Decode a JWT's (middle) payload segment into JSON claims. Display-only —
/// no signature verification.
fn jwt_claims(token: &str) -> Option<serde_json::Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = crate::account::decode_b64url(payload)?;
    serde_json::from_slice(&bytes).ok()
}

// --- response JSON (only the fields we read; everything else is ignored) -----

#[derive(Deserialize)]
struct Payload {
    plan_type: Option<String>,
    rate_limit: Option<RateLimit>,
    /// Model-scoped caps (e.g. a per-model weekly limit), named by
    /// `limit_name`.
    additional_rate_limits: Option<Vec<AdditionalLimit>>,
    credits: Option<Credits>,
    spend_control: Option<SpendControl>,
}

#[derive(Deserialize)]
struct RateLimit {
    primary_window: Option<LimitWindow>,
    secondary_window: Option<LimitWindow>,
}

#[derive(Deserialize)]
struct AdditionalLimit {
    limit_name: Option<String>,
    rate_limit: Option<RateLimit>,
}

#[derive(Deserialize)]
struct LimitWindow {
    used_percent: Option<f64>, // 0..100
    limit_window_seconds: Option<i64>,
    reset_at: Option<i64>, // Unix epoch seconds
}

#[derive(Deserialize)]
struct Credits {
    has_credits: Option<bool>,
    unlimited: Option<bool>,
    /// Numeric string (e.g. `"0"`), same quirk the rollout snapshot has.
    balance: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct SpendControl {
    reached: Option<bool>,
}

impl Payload {
    fn into_report(self) -> Result<UsageReport, String> {
        let mut windows: Vec<UsageWindow> = Vec::new();

        if let Some(rl) = self.rate_limit {
            windows.extend(rl.into_windows(None));
        }
        for extra in self.additional_rate_limits.into_iter().flatten() {
            let Some(rl) = extra.rate_limit else { continue };
            windows.extend(rl.into_windows(extra.limit_name.as_deref()));
        }

        if windows.is_empty() {
            return Err("Codex isn't rate-limited here (likely API-key billing).".into());
        }

        // Same labels/logic as the rollout path (`codexstats::build_quota`) so
        // the details panel reads identically whichever source produced it.
        let mut details = Vec::new();
        if let Some(credits) = self.credits {
            let balance = credits.balance.as_ref().and_then(parse_number);
            if credits.unlimited == Some(true) {
                details.push(UsageDetail {
                    label: "Credits".into(),
                    value: "Unlimited".into(),
                });
            } else if let Some(balance) = balance {
                details.push(UsageDetail {
                    label: "Credit balance".into(),
                    value: format_number(balance),
                });
            } else if credits.has_credits == Some(false) {
                details.push(UsageDetail {
                    label: "Credits".into(),
                    value: "No add-on balance".into(),
                });
            }
        }
        if self.spend_control.and_then(|s| s.reached) == Some(true) {
            details.push(UsageDetail {
                label: "Spend control".into(),
                value: "Reached".into(),
            });
        }

        let plan = self.plan_type.unwrap_or_else(|| "Codex".into());
        Ok(UsageReport {
            windows,
            source_label: format!("Codex limits · {plan}"),
            details,
        })
    }
}

impl RateLimit {
    /// `scope` is the model name for `additional_rate_limits` entries; scoped
    /// titles use the same "Weekly (Model)" shape as Claude's per-model caps so
    /// `usage::is_model_scoped_title` keeps them out of the device-share fit.
    fn into_windows(self, scope: Option<&str>) -> Vec<UsageWindow> {
        [self.primary_window, self.secondary_window]
            .into_iter()
            .flatten()
            .map(|w| w.into_window(scope))
            .collect()
    }
}

impl LimitWindow {
    fn into_window(self, scope: Option<&str>) -> UsageWindow {
        let util = clamp01(self.used_percent.unwrap_or(0.0) / 100.0);
        let base = window_title(self.limit_window_seconds);
        let title = match scope {
            Some(name) => format!("{base} ({name})"),
            None => base,
        };
        UsageWindow {
            utilization: util,
            remaining: clamp01(1.0 - util),
            resets_at: self.reset_at.and_then(crate::codexstats::epoch_to_rfc3339),
            title,
            window_minutes: self.limit_window_seconds.map(|s| s / 60),
            trailing: None,
            eta_secs: None,
            machine_share: None,
            others_share: None,
            share_confidence: None,
            window_budget: None,
        }
    }
}

/// Number that may arrive as a JSON number or a numeric string (`"123.5"`).
fn parse_number(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn format_number(v: f64) -> String {
    if (v.fract()).abs() < 1e-9 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

/// Map a window length to the friendly titles the bars use.
fn window_title(seconds: Option<i64>) -> String {
    match seconds {
        Some(s) if s <= 6 * 3600 => format!("{}-Hour", (s as f64 / 3600.0).round() as i64),
        Some(s) if s <= 7 * 86_400 => "Weekly".into(),
        Some(_) => "Monthly".into(),
        None => "Limit".into(),
    }
}

fn clamp01(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_real_response_shape() {
        // Trimmed from a live /wham/usage response (Codex CLI 0.145).
        let payload: Payload = serde_json::from_str(
            r#"{
                "plan_type": "prolite",
                "rate_limit": {
                    "allowed": true,
                    "primary_window": {
                        "used_percent": 9,
                        "limit_window_seconds": 604800,
                        "reset_after_seconds": 536342,
                        "reset_at": 1785829250
                    },
                    "secondary_window": null
                },
                "additional_rate_limits": [
                    {
                        "limit_name": "GPT-5.3-Codex-Spark",
                        "metered_feature": "codex_bengalfox",
                        "rate_limit": {
                            "primary_window": {
                                "used_percent": 0,
                                "limit_window_seconds": 604800,
                                "reset_at": 1785897709
                            },
                            "secondary_window": null
                        }
                    }
                ],
                "credits": {
                    "has_credits": false,
                    "unlimited": false,
                    "overage_limit_reached": false,
                    "balance": "0"
                },
                "spend_control": { "reached": false, "individual_limit": null }
            }"#,
        )
        .expect("payload parses");
        let report = payload.into_report().expect("report builds");
        assert_eq!(report.source_label, "Codex limits · prolite");
        let titles: Vec<&str> = report.windows.iter().map(|w| w.title.as_str()).collect();
        assert_eq!(titles, vec!["Weekly", "Weekly (GPT-5.3-Codex-Spark)"]);
        assert!((report.windows[0].utilization - 0.09).abs() < 1e-9);
        assert_eq!(report.windows[0].window_minutes, Some(10_080));
        assert_eq!(
            report.windows[0].resets_at.as_deref(),
            Some("2026-08-04T07:40:50+00:00")
        );
        assert!(crate::usage::is_model_scoped_title(&report.windows[1].title));
        // Zero string balance renders as a "Credit balance" row, same as the
        // rollout path.
        assert_eq!(report.details[0].label, "Credit balance");
        assert_eq!(report.details[0].value, "0");
    }

    #[test]
    fn all_null_windows_read_as_api_key_billing() {
        let payload: Payload = serde_json::from_str(
            r#"{ "plan_type": null, "rate_limit": { "primary_window": null, "secondary_window": null } }"#,
        )
        .expect("payload parses");
        assert!(payload.into_report().is_err());
    }

    #[test]
    fn window_titles_by_length() {
        assert_eq!(window_title(Some(5 * 3600)), "5-Hour");
        assert_eq!(window_title(Some(604_800)), "Weekly");
        assert_eq!(window_title(Some(30 * 86_400)), "Monthly");
        assert_eq!(window_title(None), "Limit");
    }
}
