//! Per-model token usage and rate-limit quota from OpenAI Codex CLI's local
//! session rollouts (`~/.codex/sessions/<YYYY>/<MM>/<DD>/rollout-*.jsonl`).
//!
//! Codex's rollout is event-shaped (unlike Claude Code's `message.usage` rows).
//! The two events we care about:
//!   - `turn_context` carries `model` — we track it as the "current model".
//!   - `event_msg` of type `token_count` carries, per turn:
//!       payload.info.last_token_usage  — that turn's token delta
//!       payload.rate_limits            — a quota snapshot (used_percent, etc.)
//!
//! Local usage sums `last_token_usage` per model within the time window. Live
//! quota reads the most recent non-null `rate_limits` across all sessions — no
//! network call, we just read what Codex already wrote.

use crate::localstats::{ModelCostDTO, ModelUsageDTO};
use crate::pricing::Pricing;
use crate::usage::{UsageReport, UsageWindow};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const TOKEN_COUNT_MARKER: &str = "\"type\":\"token_count\"";

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
                    let Some(usage) = payload.info.as_ref().and_then(|i| i.last_token_usage.as_ref())
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
            // OpenAI accounting: `input_tokens` is the full input, of which
            // `cached_input_tokens` was served from cache. Split them into our
            // uncached-input / cache-read buckets. There's no cache-write token
            // category, so cache_create stays 0. Reasoning tokens bill as output.
            let input = (t.input - t.cached).max(0);
            let cache_read = t.cached;
            let output = t.output + t.reasoning;
            let cost = pricing.cost(&id, input, output, cache_read, 0, 0).map(|c| {
                ModelCostDTO {
                    input: c.input,
                    output: c.output,
                    cache_read: c.cache_read,
                    cache_create: c.cache_create,
                    total: c.total(),
                }
            });
            let total = input + output + cache_read;
            let max_component = input.max(output).max(cache_read);
            ModelUsageDTO {
                display_name: display_name(&id),
                id,
                input,
                output,
                cache_read,
                cache_create: 0,
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

/// The freshest Codex quota snapshot, shaped like the live-quota hearts bars.
/// Rollout filenames embed an ISO timestamp, so we scan newest-first and stop at
/// the first session carrying a `rate_limits` snapshot rather than reading every
/// file.
pub fn fetch_quota() -> Result<UsageReport, String> {
    let mut files = session_files();
    if files.is_empty() {
        return Err("No local Codex sessions found under ~/.codex/sessions.".into());
    }
    files.sort();
    files.reverse(); // newest first

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
        if let Some((_, limits)) = best {
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

    let plan = limits.plan_type.unwrap_or_else(|| "Codex".into());
    Ok(UsageReport {
        windows,
        source_label: format!("Codex limits · {plan}"),
    })
}

#[derive(Default)]
struct Totals {
    input: i64,
    cached: i64,
    output: i64,
    reasoning: i64,
}

impl Totals {
    fn add(&mut self, u: &TokenUsage) {
        self.input += u.input_tokens.unwrap_or(0);
        self.cached += u.cached_input_tokens.unwrap_or(0);
        self.output += u.output_tokens.unwrap_or(0);
        self.reasoning += u.reasoning_output_tokens.unwrap_or(0);
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
    reasoning_output_tokens: Option<i64>,
}

#[derive(Deserialize, Clone)]
struct RateLimits {
    primary: Option<Window>,
    secondary: Option<Window>,
    plan_type: Option<String>,
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
            trailing: None,
        }
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

fn epoch_to_rfc3339(secs: i64) -> Option<String> {
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
    DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.timestamp())
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
    v.max(0.0).min(1.0)
}
