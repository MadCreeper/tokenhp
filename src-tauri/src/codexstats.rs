//! Per-model token usage and rate-limit quota from OpenAI Codex CLI's local
//! session rollouts (`~/.codex/sessions/<YYYY>/<MM>/<DD>/rollout-*.jsonl`).
//!
//! Codex's rollout is event-shaped (unlike Claude Code's `message.usage` rows).
//! The two events we care about:
//!   - `turn_context` carries `model` — we track it as the "current model".
//!   - `event_msg` of type `token_count` carries, per turn:
//!     payload.info.last_token_usage  — that turn's token delta
//!     payload.rate_limits            — a quota snapshot (used_percent, etc.)
//!
//! Local usage sums `last_token_usage` per model within the time window. Live
//! quota reads the most recent non-null `rate_limits` across all sessions — no
//! network call, we just read what Codex already wrote.

use crate::localstats::{ModelCostDTO, ModelUsageDTO, UsageRow};
use crate::pricing::Pricing;
use crate::usage::{UsageDetail, UsageReport, UsageWindow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const TOKEN_COUNT_MARKER: &str = "\"type\":\"token_count\"";

/// How stale a local rollout snapshot may be before [`fetch_quota`] rejects it.
pub const SNAPSHOT_MAX_AGE_SECS: i64 = 24 * 3600;

/// Per-model Codex token usage over the last `window_secs`. Returns an empty
/// vec when there's nothing (the `tools` layer decides what "empty" means);
/// this is the Codex [`crate::tools::ToolAdapter`] implementation's data source.
pub fn collect_local(window_secs: i64) -> Vec<ModelUsageDTO> {
    let now = Utc::now().timestamp();
    let mut totals: HashMap<String, Totals> = HashMap::new();

    for file in &session_files() {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        // Model is announced in `turn_context` and applies to the token_count
        // events that follow it, within the same session file.
        let mut current_model: Option<String> = None;
        for line in text.lines() {
            let Ok(row) = serde_json::from_str::<Row>(line) else {
                continue;
            };
            match row.r#type.as_deref() {
                Some("turn_context") => {
                    if let Some(m) = row.payload.as_ref().and_then(|p| p.model.clone()) {
                        current_model = Some(m);
                    }
                }
                Some("event_msg") => {
                    // Cheap guard before trusting the parsed payload.
                    if !line.contains(TOKEN_COUNT_MARKER) {
                        continue;
                    }
                    let Some(payload) = row.payload.as_ref() else {
                        continue;
                    };
                    if payload.r#type.as_deref() != Some("token_count") {
                        continue;
                    }
                    let Some(ts) = row.timestamp.as_deref().and_then(parse_ts) else {
                        continue;
                    };
                    let age = now - ts;
                    if age < 0 || age >= window_secs {
                        continue;
                    }
                    let Some(usage) = payload
                        .info
                        .as_ref()
                        .and_then(|i| i.last_token_usage.as_ref())
                    else {
                        continue;
                    };
                    // Drop usage we can't attribute to a model (a token_count
                    // before any turn_context) rather than inventing an "unknown".
                    let Some(model) = current_model.clone() else {
                        continue;
                    };
                    totals.entry(model).or_default().add(usage);
                }
                _ => {}
            }
        }
    }

    let pricing = Pricing::loaded();
    let mut models: Vec<ModelUsageDTO> = totals
        .into_iter()
        .map(|(id, t)| {
            // `input_tokens` is the full input, of which cached input is a
            // subset. `reasoning_output_tokens` is likewise already included
            // in `output_tokens`; adding it again inflates every reasoning turn.
            let input = (t.input - t.cached).max(0);
            let cache_read = t.cached;
            let output = t.output;
            let cost = pricing
                .cost(&id, input, output, cache_read, 0, 0)
                .map(|c| ModelCostDTO {
                    input: c.input,
                    output: c.output,
                    cache_read: c.cache_read,
                    cache_create: c.cache_create,
                    total: c.total(),
                });
            let total = t.total.max(input + output + cache_read);
            let unattributed = (total - input - output - cache_read).max(0);
            let max_component = input.max(output).max(cache_read).max(unattributed);
            ModelUsageDTO {
                display_name: display_name(&id),
                id,
                input,
                output,
                cache_read,
                cache_create: 0,
                unattributed,
                total,
                max_component,
                cost,
            }
        })
        .filter(|m| m.total > 0) // drop models that logged a turn but no tokens
        .collect();

    models.sort_by(|a, b| b.total.cmp(&a.total));
    models
}

/// Exact per-(UTC day, account, project, model) Codex usage for Team sharing.
/// Session metadata supplies the working directory; token_count events supply
/// the per-turn delta and timestamp.
pub fn collect_rows(start_day: &str, end_day: &str) -> Vec<UsageRow> {
    type Key = (String, String, String, String, String, String, String);
    let mut acc: HashMap<Key, (Totals, i64)> = HashMap::new();
    let attributions = crate::account::attribution_snapshot();

    for file in &session_files() {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        let mut current_model: Option<String> = None;
        let mut current_project = "unknown".to_string();
        for line in text.lines() {
            let Ok(row) = serde_json::from_str::<Row>(line) else {
                continue;
            };
            match row.r#type.as_deref() {
                Some("session_meta") => {
                    if let Some(cwd) = row.payload.as_ref().and_then(|p| p.cwd.as_deref()) {
                        current_project = crate::localstats::project_name(cwd);
                    }
                }
                Some("turn_context") => {
                    if let Some(model) = row.payload.as_ref().and_then(|p| p.model.clone()) {
                        current_model = Some(model);
                    }
                }
                Some("event_msg") if line.contains(TOKEN_COUNT_MARKER) => {
                    let Some(payload) = row.payload.as_ref() else {
                        continue;
                    };
                    if payload.r#type.as_deref() != Some("token_count") {
                        continue;
                    }
                    let Some(ts) = row.timestamp.as_deref().and_then(parse_ts) else {
                        continue;
                    };
                    let Some(day) = DateTime::<Utc>::from_timestamp(ts, 0)
                        .map(|dt| dt.format("%Y-%m-%d").to_string())
                    else {
                        continue;
                    };
                    if day.as_str() < start_day || day.as_str() > end_day {
                        continue;
                    }
                    let Some(model) = current_model.clone() else {
                        continue;
                    };
                    let Some(usage) = payload
                        .info
                        .as_ref()
                        .and_then(|i| i.last_token_usage.as_ref())
                    else {
                        continue;
                    };
                    let attribution = attributions.attribution("codex", ts);
                    let key = (
                        day,
                        current_project.clone(),
                        model,
                        attribution.account_key,
                        attribution.billing_key,
                        attribution.account_label,
                        attribution.status,
                    );
                    let entry = acc.entry(key).or_insert_with(|| (Totals::default(), 0));
                    entry.0.add(usage);
                    entry.1 = entry.1.max(ts);
                }
                _ => {}
            }
        }
    }

    let pricing = Pricing::loaded();
    let mut rows = Vec::new();
    for (
        (day, project, model, account_key, billing_key, account_label, attribution_status),
        (totals, last_active),
    ) in acc
    {
        let input = (totals.input - totals.cached).max(0);
        let cache_read = totals.cached;
        let output = totals.output;
        let tokens = totals.total.max(input + cache_read + output);
        let unattributed = (tokens - input - cache_read - output).max(0);
        let cost = pricing
            .cost(&model, input, output, cache_read, 0, 0)
            .map(|c| c.total())
            .unwrap_or(0.0);
        rows.push(UsageRow {
            day,
            provider: "codex".into(),
            account_key,
            billing_key,
            account_label,
            attribution_status,
            project,
            model,
            input,
            output,
            cache_read,
            cache_create: 0,
            unattributed,
            tokens,
            cost,
            last_active,
        });
    }
    rows.sort_by(|a, b| a.day.cmp(&b.day).then(b.tokens.cmp(&a.tokens)));
    rows
}

/// The freshest Codex quota snapshot, shaped like the live-quota hearts bars.
/// We scan newest-first and stop at the first session carrying a `rate_limits`
/// snapshot rather than reading every file.
///
/// Offline fallback only — Codex writes these snapshots just when a real turn
/// completes (see `codexquota` for the live source), so a snapshot older than
/// `max_age_secs` is rejected rather than shown as if it were current (an idle
/// machine once surfaced a six-week-old snapshot as live data).
pub fn fetch_quota(max_age_secs: i64) -> Result<UsageReport, String> {
    let mut files = session_files();
    if files.is_empty() {
        return Err("No local Codex sessions found under ~/.codex/sessions.".into());
    }
    // Long-running sessions can remain active after a newer short-lived
    // subagent exits, so filename creation time is not sufficient. File mtime
    // identifies the rollout most recently updated by Codex.
    files.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
    });
    files.reverse(); // most recently written first

    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        // Most recent rate_limits snapshot within this (newest) session.
        let mut best: Option<(i64, RateLimits)> = None;
        for line in text.lines() {
            if !line.contains(TOKEN_COUNT_MARKER) {
                continue;
            }
            let Ok(row) = serde_json::from_str::<Row>(line) else {
                continue;
            };
            let Some(limits) = row.payload.as_ref().and_then(|p| p.rate_limits.clone()) else {
                continue;
            };
            let Some(ts) = row.timestamp.as_deref().and_then(parse_ts) else {
                continue;
            };
            if best.as_ref().map_or(true, |(b, _)| ts > *b) {
                best = Some((ts, limits));
            }
        }
        if let Some((ts, limits)) = best {
            // Files are scanned newest-first, so anything past this one is
            // older still — bail rather than keep digging.
            if Utc::now().timestamp() - ts > max_age_secs {
                return Err("Codex hasn't written a local rate-limit snapshot recently.".into());
            }
            return build_quota(limits);
        }
    }

    Err("No Codex rate-limit data yet — run a recent Codex session first.".into())
}

fn build_quota(limits: RateLimits) -> Result<UsageReport, String> {
    let windows: Vec<UsageWindow> = [
        limits.primary.map(|w| w.into_window()),
        limits.secondary.map(|w| w.into_window()),
    ]
    .into_iter()
    .flatten()
    .collect();

    if windows.is_empty() {
        // rate_limits object present but all-null — typical of API-key auth,
        // which is metered per-token rather than rate-limited by a quota.
        return Err("Codex isn't rate-limited here (likely API-key billing).".into());
    }

    let mut details = Vec::new();
    if let Some(credits) = limits.credits {
        if credits.unlimited == Some(true) {
            details.push(UsageDetail {
                label: "Credits".into(),
                value: "Unlimited".into(),
            });
        } else if let Some(balance) = credits.balance {
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
    if limits.spend_control_reached == Some(true) {
        details.push(UsageDetail {
            label: "Spend control".into(),
            value: "Reached".into(),
        });
    }
    if let Some(name) = limits.limit_name.filter(|s| !s.trim().is_empty()) {
        details.push(UsageDetail {
            label: "Limit".into(),
            value: name,
        });
    }
    let plan = limits.plan_type.unwrap_or_else(|| "Codex".into());
    Ok(UsageReport {
        windows,
        source_label: format!("Codex limits · {plan}"),
        details,
    })
}

#[derive(Default)]
struct Totals {
    input: i64,
    cached: i64,
    output: i64,
    total: i64,
}

impl Totals {
    fn add(&mut self, u: &TokenUsage) {
        let input = u.input_tokens.unwrap_or(0);
        let output = u.output_tokens.unwrap_or(0);
        self.input += input;
        self.cached += u.cached_input_tokens.unwrap_or(0);
        self.output += output;
        self.total += u.total_tokens.unwrap_or(input + output);
    }
}

// --- rollout JSON (only the fields we read; everything else is ignored) ---

#[derive(Deserialize)]
struct Row {
    r#type: Option<String>,
    timestamp: Option<String>,
    payload: Option<Payload>,
}

#[derive(Deserialize)]
struct Payload {
    r#type: Option<String>,
    /// Present on `turn_context`.
    model: Option<String>,
    /// Present on session_meta.
    cwd: Option<String>,
    /// Present on `token_count` events.
    info: Option<Info>,
    rate_limits: Option<RateLimits>,
}

#[derive(Deserialize)]
struct Info {
    last_token_usage: Option<TokenUsage>,
}

#[derive(Deserialize)]
struct TokenUsage {
    input_tokens: Option<i64>,
    cached_input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    total_tokens: Option<i64>,
    // Parsed for forward compatibility/documentation. This is a subset of
    // output_tokens and must never be added to the total a second time.
    #[allow(dead_code)]
    reasoning_output_tokens: Option<i64>,
}

#[derive(Deserialize, Clone)]
struct RateLimits {
    primary: Option<Window>,
    secondary: Option<Window>,
    plan_type: Option<String>,
    credits: Option<Credits>,
    spend_control_reached: Option<bool>,
    limit_name: Option<String>,
}

#[derive(Deserialize, Clone)]
struct Credits {
    has_credits: Option<bool>,
    unlimited: Option<bool>,
    /// Codex 0.145 writes this as `"0"` while older versions used a JSON
    /// number. Accept both so one cosmetic field cannot discard the whole
    /// token_count event (including its usage and quota).
    #[serde(default, deserialize_with = "optional_number")]
    balance: Option<f64>,
}

fn optional_number<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(n)) => Ok(n.as_f64()),
        Some(serde_json::Value::String(s)) => {
            s.parse::<f64>().map(Some).map_err(serde::de::Error::custom)
        }
        Some(other) => Err(serde::de::Error::custom(format!(
            "expected number or numeric string, got {other}"
        ))),
    }
}

#[derive(Deserialize, Clone)]
struct Window {
    used_percent: Option<f64>,
    window_minutes: Option<i64>,
    resets_at: Option<i64>, // Unix epoch seconds
}

impl Window {
    fn into_window(self) -> UsageWindow {
        let util = clamp01(self.used_percent.unwrap_or(0.0) / 100.0);
        UsageWindow {
            utilization: util,
            remaining: clamp01(1.0 - util),
            resets_at: self.resets_at.and_then(epoch_to_rfc3339),
            title: window_title(self.window_minutes),
            window_minutes: self.window_minutes,
            trailing: None,
            eta_secs: None,
            machine_share: None,
            others_share: None,
            share_confidence: None,
            window_budget: None,
        }
    }
}

fn format_number(v: f64) -> String {
    if (v.fract()).abs() < 1e-9 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

/// Map a rate-limit window length to a friendly title.
fn window_title(minutes: Option<i64>) -> String {
    match minutes {
        Some(m) if m <= 360 => format!("{}-Hour", (m as f64 / 60.0).round() as i64),
        Some(m) if m <= 10_080 => "Weekly".into(),
        Some(_) => "Monthly".into(),
        None => "Limit".into(),
    }
}

pub(crate) fn epoch_to_rfc3339(secs: i64) -> Option<String> {
    DateTime::<Utc>::from_timestamp(secs, 0).map(|dt| dt.to_rfc3339())
}

/// Collect every rollout `*.jsonl` under `~/.codex/sessions`.
fn session_files() -> Vec<PathBuf> {
    let dir = dirs::home_dir()
        .map(|h| h.join(".codex").join("sessions"))
        .unwrap_or_default();
    let mut out = Vec::new();
    collect_jsonl(&dir, &mut out);
    out
}

fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

fn parse_ts(s: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp())
}

/// `gpt-5.5` → "GPT-5.5"; other ids pass through.
fn display_name(id: &str) -> String {
    if let Some(rest) = id.strip_prefix("gpt") {
        format!("GPT{rest}")
    } else {
        id.to_string()
    }
}

fn clamp01(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_is_not_added_twice() {
        let mut totals = Totals::default();
        totals.add(&TokenUsage {
            input_tokens: Some(50),
            cached_input_tokens: Some(20),
            output_tokens: Some(100),
            total_tokens: Some(150),
            reasoning_output_tokens: Some(80),
        });
        assert_eq!(totals.input, 50);
        assert_eq!(totals.output, 100);
        assert_eq!(totals.total, 150);
    }

    #[test]
    fn weekly_only_quota_and_credit_balance_are_preserved() {
        let limits: RateLimits = serde_json::from_str(
            r#"{
              "primary":{"used_percent":42,"window_minutes":10080,"resets_at":1786000000},
              "secondary":null,
              "plan_type":"prolite",
              "credits":{"has_credits":true,"unlimited":false,"balance":"123.5"}
            }"#,
        )
        .unwrap();
        let report = build_quota(limits).unwrap();
        assert_eq!(report.windows.len(), 1);
        assert_eq!(report.windows[0].title, "Weekly");
        assert_eq!(report.windows[0].window_minutes, Some(10_080));
        assert_eq!(report.details[0].label, "Credit balance");
        assert_eq!(report.details[0].value, "123.50");
    }
}
