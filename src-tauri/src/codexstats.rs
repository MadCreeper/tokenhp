//! Per-model token usage and rate-limit quota from OpenAI Codex CLI's local
//! session rollouts (`~/.codex/sessions/<YYYY>/<MM>/<DD>/rollout-*.jsonl`).
//!
//! Codex's rollout is event-shaped (unlike Claude Code's `message.usage` rows).
//! Usage comes from request-level `token_usage_record` events in newer logs,
//! or cumulative `token_count` deltas in older logs. Both can describe the same
//! request, so they must be reconciled before aggregating. Model attribution
//! follows both turn contexts and thread settings, without a model allowlist.
//! Live quota reads the most recent non-null `rate_limits` snapshot.

use crate::localstats::{ModelCostDTO, ModelUsageDTO, UsageRow};
use crate::pricing::{ModelCost, Pricing};
use crate::usage::{UsageDetail, UsageReport, UsageWindow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const TOKEN_COUNT_MARKER: &str = "token_count";

/// Bump when accounting changes, so device-share fitting cannot mix costs
/// collected by incompatible parsers within the same quota window.
pub(crate) const PARSER_REVISION: u32 = 2;

/// How stale a local rollout snapshot may be before [`fetch_quota`] rejects it.
pub const SNAPSHOT_MAX_AGE_SECS: i64 = 24 * 3600;

/// Per-model Codex token usage over the last `window_secs`. Returns an empty
/// vec when there's nothing (the `tools` layer decides what "empty" means);
/// this is the Codex [`crate::tools::ToolAdapter`] implementation's data source.
pub fn collect_local(window_secs: i64) -> Vec<ModelUsageDTO> {
    let now = Utc::now().timestamp();
    collect_local_range(now - window_secs, now)
}

/// Per-model Codex token usage for turns with `start <= timestamp < end` (unix
/// seconds). The device-share fit uses this to price the span a provider
/// window covered *as of* an observation, which need not end now.
pub fn collect_local_range(start: i64, end: i64) -> Vec<ModelUsageDTO> {
    collect_local_range_for_account(start, end, None).unwrap_or_default()
}

pub fn collect_local_range_for_account(
    start: i64,
    end: i64,
    account: Option<&str>,
) -> Option<Vec<ModelUsageDTO>> {
    let attribution = account.map(|_| crate::account::attribution_snapshot());
    let mut totals: HashMap<String, Totals> = HashMap::new();

    let pricing = Pricing::loaded();
    for event in collect_events(&session_files()) {
        if event.ts < start || event.ts >= end {
            continue;
        }
        if let (Some(key), Some(snapshot)) = (account, &attribution) {
            let identity = snapshot.attribution("codex", event.ts);
            if identity.status != "exact" {
                return None;
            }
            if identity.account_key != key {
                continue;
            }
        }
        totals.entry(event.model.clone()).or_default().add_priced(
            &event.usage,
            &event.model,
            &pricing,
            event.ts,
        );
    }

    let mut models: Vec<ModelUsageDTO> = totals
        .into_iter()
        .map(|(id, t)| {
            // `input_tokens` is the full input, of which cached input is a
            // subset. `reasoning_output_tokens` is likewise already included
            // in `output_tokens`; adding it again inflates every reasoning turn.
            let input = (t.input - t.cached - t.written).max(0);
            let cache_read = t.cached;
            let output = t.output;
            let cost = t.cost().map(|c| ModelCostDTO {
                input: c.input,
                output: c.output,
                cache_read: c.cache_read,
                cache_create: c.cache_create,
                total: c.total(),
            });
            let total = t.total.max(input + output + cache_read + t.written);
            let unattributed = (total - input - output - cache_read - t.written).max(0);
            let max_component = input
                .max(output)
                .max(cache_read)
                .max(t.written)
                .max(unattributed);
            ModelUsageDTO {
                display_name: display_name(&id),
                id,
                input,
                output,
                cache_read,
                cache_create: t.written,
                unattributed,
                total,
                max_component,
                cost,
            }
        })
        .filter(|m| m.total > 0) // drop models that logged a turn but no tokens
        .collect();

    models.sort_by(|a, b| b.total.cmp(&a.total));
    Some(models)
}

/// Exact per-(UTC day, account, project, model) Codex usage for Team sharing.
/// Uses the same normalized requests as local bars and device-share fitting.
pub fn collect_rows(start_day: &str, end_day: &str) -> Vec<UsageRow> {
    type Key = (String, String, String, String, String, String, String);
    let mut acc: HashMap<Key, (Totals, i64)> = HashMap::new();
    let attributions = crate::account::attribution_snapshot();

    let pricing = Pricing::loaded();
    for event in collect_events(&session_files()) {
        let Some(day) = DateTime::<Utc>::from_timestamp(event.ts, 0)
            .map(|dt| dt.format("%Y-%m-%d").to_string())
        else {
            continue;
        };
        if day.as_str() < start_day || day.as_str() > end_day {
            continue;
        }
        let attribution = attributions.attribution("codex", event.ts);
        let key = (
            day,
            event.project,
            event.model.clone(),
            attribution.account_key,
            attribution.billing_key,
            attribution.account_label,
            attribution.status,
        );
        let entry = acc.entry(key).or_insert_with(|| (Totals::default(), 0));
        entry
            .0
            .add_priced(&event.usage, &event.model, &pricing, event.ts);
        entry.1 = entry.1.max(event.ts);
    }

    let mut rows = Vec::new();
    for (
        (day, project, model, account_key, billing_key, account_label, attribution_status),
        (totals, last_active),
    ) in acc
    {
        let input = (totals.input - totals.cached - totals.written).max(0);
        let cache_read = totals.cached;
        let output = totals.output;
        let tokens = totals
            .total
            .max(input + cache_read + output + totals.written);
        let unattributed = (tokens - input - cache_read - output - totals.written).max(0);
        let cost = totals.cost().map(|c| c.total()).unwrap_or(0.0);
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
            cache_create: totals.written,
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
    fetch_quota_with_ts(max_age_secs).map(|(_, report)| report)
}

/// [`fetch_quota`] plus the unix time the snapshot was written. Anything that
/// *records* the figures (burn history, device-share samples) must stamp them
/// with this, not with "now": the snapshot describes the account as of that
/// turn, and re-reading it on every poll is one observation, not many.
pub fn fetch_quota_with_ts(max_age_secs: i64) -> Result<(i64, UsageReport), String> {
    let mut files = session_files();
    if files.is_empty() {
        return Err("No local Codex sessions found under ~/.codex/sessions.".into());
    }
    // Long-running sessions can remain active after a newer short-lived
    // subagent exits, so filename creation time is not sufficient. File mtime
    // identifies the rollout most recently updated by Codex.
    files.sort_by_key(|path| std::fs::metadata(path).and_then(|m| m.modified()).ok());
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
            if row.r#type.as_deref() != Some("event_msg")
                || row.payload.as_ref().and_then(|p| p.r#type.as_deref()) != Some("token_count")
            {
                continue;
            }
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
            return build_quota(limits).map(|report| (ts, report));
        }
    }

    Err("No Codex rate-limit data yet — run a recent Codex session first.".into())
}

fn build_quota(limits: RateLimits) -> Result<UsageReport, String> {
    let scope = limits.limit_name.filter(|s| !s.trim().is_empty());
    let windows: Vec<UsageWindow> = [
        limits.primary.map(|w| w.into_window(scope.as_deref())),
        limits.secondary.map(|w| w.into_window(scope.as_deref())),
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
        if credits.has_credits == Some(false) {
            // No add-on balance means there is nothing useful to display.
        } else if credits.unlimited == Some(true) {
            details.push(UsageDetail {
                label: "Credits".into(),
                value: "Unlimited".into(),
            });
        } else if let Some(balance) = credits.balance {
            details.push(UsageDetail {
                label: "Credit balance".into(),
                value: format_number(balance),
            });
        }
    }
    if limits.spend_control_reached == Some(true) {
        details.push(UsageDetail {
            label: "Spend control".into(),
            value: "Reached".into(),
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
    written: i64,
    priced: ModelCost,
    unpriced: bool,
    input: i64,
    cached: i64,
    output: i64,
    total: i64,
}

impl Totals {
    fn cost(&self) -> Option<ModelCost> {
        if self.unpriced {
            None
        } else {
            Some(self.priced)
        }
    }

    fn add_priced(&mut self, u: &TokenUsage, model: &str, pricing: &Pricing, at: i64) {
        self.add(u);
        let mut t = Totals::default();
        t.add(u);
        // A total without its input/output split cannot be priced accurately.
        self.unpriced |= t.total > t.input + t.output;
        match pricing.cost_request(
            model,
            [
                t.input - t.cached - t.written,
                t.output,
                t.cached,
                t.written,
                0,
            ],
            at,
        ) {
            Some(c) => self.priced.add(c),
            None => self.unpriced = true,
        }
    }

    fn add(&mut self, u: &TokenUsage) {
        let input = u.input_tokens.unwrap_or(0).max(0);
        let output = u.output_tokens.unwrap_or(0).max(0);
        self.input += input;
        let cached = u.cached_input_tokens.unwrap_or(0).clamp(0, input);
        self.cached += cached;
        self.written += u
            .cache_write_input_tokens
            .unwrap_or(0)
            .clamp(0, input - cached);
        self.output += output;
        self.total += u.total_tokens.unwrap_or(input + output).max(input + output);
    }
}

#[derive(Debug)]
struct CodexEvent {
    ts: i64,
    project: String,
    model: String,
    usage: TokenUsage,
    request_id: Option<(String, String)>,
}

fn collect_events(files: &[PathBuf]) -> Vec<CodexEvent> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for file in files {
        let Ok(bytes) = std::fs::read(file) else {
            continue;
        };
        // A partially written UTF-8 tail must not hide the complete history.
        let text = String::from_utf8_lossy(&bytes);
        for event in parse_events(&text) {
            // Resumed/forked rollouts can carry an already recorded response.
            if let Some(id) = &event.request_id {
                if !seen.insert(id.clone()) {
                    continue;
                }
            }
            out.push(event);
        }
    }
    out
}

/// Parse the entire rollout before applying a date/account filter: cumulative
/// baselines and model settings often precede the requested range by days.
fn parse_events(text: &str) -> Vec<CodexEvent> {
    let mut out = Vec::new();
    let mut model = "unknown".to_string();
    let mut project = "unknown".to_string();
    let mut thread_id = String::new();
    let mut counter = UsageCounter::default();
    let mut seen = HashSet::new();
    let mut pending: Vec<TokenUsage> = Vec::new();
    let mut compacted = false;
    for line in text.lines() {
        // Decode usage separately from quota. An unrelated rate-limit schema
        // change must not discard a perfectly valid request's token counts.
        let Ok(row) = serde_json::from_str::<UsageLogRow>(line) else {
            continue;
        };
        let p = &row.payload;
        match row.r#type.as_str() {
            "session_meta" | "turn_context" => {
                if let Some(m) = settings_model(p) {
                    model = m.to_string();
                }
                if let Some(cwd) = nonempty_string(p.get("cwd")) {
                    project = crate::localstats::project_name(cwd);
                }
                if row.r#type == "session_meta" {
                    thread_id = nonempty_string(p.get("id")).unwrap_or("").to_string();
                }
            }
            "compacted" => compacted = true,
            "thread_settings_applied" | "event_msg"
                if row.r#type == "thread_settings_applied"
                    || p.get("type").and_then(|v| v.as_str())
                        == Some("thread_settings_applied") =>
            {
                if let Some(m) = p.get("thread_settings").and_then(settings_model) {
                    model = m.to_string();
                }
            }
            "token_usage_record" | "event_msg" => {
                let detailed = row.r#type == "token_usage_record"
                    || p.get("type").and_then(|v| v.as_str()) == Some("token_usage_record");
                let (usage, request_id) = if detailed {
                    // Leave the legacy counter available as fallback if this
                    // record cannot be placed in time.
                    if row.timestamp.as_deref().and_then(parse_ts).is_none() {
                        continue;
                    }
                    let Some(usage) = p.get("usage").and_then(decode_usage) else {
                        continue;
                    };
                    if !usage.has_tokens() {
                        continue;
                    }
                    let id = nonempty_string(p.get("response_id")).map(|id| {
                        (
                            nonempty_string(p.get("thread_id"))
                                .unwrap_or(&thread_id)
                                .to_string(),
                            id.to_string(),
                        )
                    });
                    // A duplicate detail can also be followed by a mirrored
                    // counter, so reconcile it even when already emitted.
                    pending.push(usage.clone());
                    if let Some(id) = &id {
                        if !seen.insert(id.clone()) {
                            continue;
                        }
                    }
                    (usage, id)
                } else {
                    if p.get("type").and_then(|v| v.as_str()) != Some("token_count") {
                        continue;
                    }
                    let Some(info) = p.get("info").filter(|v| v.is_object()) else {
                        continue;
                    };
                    let info = Info {
                        total_token_usage: info.get("total_token_usage").and_then(decode_usage),
                        last_token_usage: info.get("last_token_usage").and_then(decode_usage),
                    };
                    let delta = counter.delta(&info);
                    let covered = std::mem::take(&mut pending);
                    let was_compacted = std::mem::take(&mut compacted);
                    let Some(mut usage) = delta else { continue };
                    // Compaction's residual context size is not a new API
                    // request. Its actual cost is in the detailed record.
                    if was_compacted && !usage.has_components() {
                        continue;
                    }
                    let sum = covered
                        .iter()
                        .fold(TokenUsage::default(), |sum, u| sum.plus(u));
                    if !covered.is_empty() && usage.same_counts(&sum) {
                        continue;
                    }
                    // Mixed-version/incomplete logs may have a cumulative
                    // delta containing both a detail and older counter-only
                    // requests. Retain the uncovered remainder.
                    if let Some(mirrored) = covered.iter().rev().find(|u| {
                        info.last_token_usage
                            .as_ref()
                            .is_some_and(|last| u.same_counts(last))
                            || u.same_counts(&usage)
                    }) {
                        let all_covered = !was_compacted
                            && usage
                                .counts()
                                .iter()
                                .zip(sum.counts())
                                .all(|(a, b)| *a >= b);
                        usage = usage.minus(if all_covered { &sum } else { mirrored });
                    }
                    if !usage.has_tokens() {
                        continue;
                    }
                    (usage, None)
                };
                let Some(ts) = row.timestamp.as_deref().and_then(parse_ts) else {
                    continue;
                };
                out.push(CodexEvent {
                    ts,
                    project: project.clone(),
                    // Use an explicit request model when supplied, otherwise
                    // the most recent settings. Preserve unfamiliar model ids.
                    model: settings_model(p).unwrap_or(&model).to_string(),
                    usage,
                    request_id,
                });
            }
            _ => {}
        }
    }
    out
}

#[derive(Deserialize)]
struct UsageLogRow {
    r#type: String,
    timestamp: Option<String>,
    #[serde(default)]
    payload: serde_json::Value,
}

fn nonempty_string(value: Option<&serde_json::Value>) -> Option<&str> {
    value
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn settings_model(value: &serde_json::Value) -> Option<&str> {
    nonempty_string(value.get("model"))
        .or_else(|| nonempty_string(value.pointer("/collaboration_mode/settings/model")))
}

fn decode_usage(value: &serde_json::Value) -> Option<TokenUsage> {
    let usage: TokenUsage = serde_json::from_value(value.clone()).ok()?;
    // An empty/unrecognized object is not an explicit zero counter reset.
    (usage.input_tokens.is_some() || usage.output_tokens.is_some() || usage.total_tokens.is_some())
        .then_some(usage)
}

/// Quota-only events repeat last_token_usage. Session counters distinguish
/// these from new turns (including equally-sized consecutive turns).
#[derive(Default)]
struct UsageCounter {
    previous: Option<TokenUsage>,
}

impl UsageCounter {
    fn delta(&mut self, info: &Info) -> Option<TokenUsage> {
        let Some(total) = &info.total_token_usage else {
            return info.last_token_usage.clone();
        };
        let previous = self.previous.replace(total.clone());
        // A zeroed cumulative counter plus a nonzero last total is a context
        // reset notification, not billable usage (observed after compaction).
        if !total.has_tokens() {
            return None;
        }
        let Some(previous) = previous else {
            return info
                .last_token_usage
                .clone()
                .or_else(|| Some(total.clone()));
        };
        let diff = |a: Option<i64>, b: Option<i64>| (a.unwrap_or(0) - b.unwrap_or(0)).max(0);
        // Context compaction / session restart may reset cumulative counters.
        if total.input_tokens.unwrap_or(0) < previous.input_tokens.unwrap_or(0)
            || total.output_tokens.unwrap_or(0) < previous.output_tokens.unwrap_or(0)
        {
            return info.last_token_usage.clone();
        }
        let delta = TokenUsage {
            cache_write_input_tokens: Some(diff(
                total.cache_write_input_tokens,
                previous.cache_write_input_tokens,
            )),
            input_tokens: Some(diff(total.input_tokens, previous.input_tokens)),
            cached_input_tokens: Some(diff(
                total.cached_input_tokens,
                previous.cached_input_tokens,
            )),
            output_tokens: Some(diff(total.output_tokens, previous.output_tokens)),
            total_tokens: Some(diff(total.total_tokens, previous.total_tokens)),
            reasoning_output_tokens: None,
        };
        if delta.input_tokens == Some(0)
            && delta.output_tokens == Some(0)
            && delta.total_tokens == Some(0)
        {
            return None;
        }
        Some(delta)
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
    rate_limits: Option<RateLimits>,
}

#[derive(Deserialize)]
struct Info {
    total_token_usage: Option<TokenUsage>,
    last_token_usage: Option<TokenUsage>,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct TokenUsage {
    #[serde(default, deserialize_with = "optional_token_count")]
    cache_write_input_tokens: Option<i64>,
    #[serde(default, deserialize_with = "optional_token_count")]
    input_tokens: Option<i64>,
    #[serde(default, deserialize_with = "optional_token_count")]
    cached_input_tokens: Option<i64>,
    #[serde(default, deserialize_with = "optional_token_count")]
    output_tokens: Option<i64>,
    #[serde(default, deserialize_with = "optional_token_count")]
    total_tokens: Option<i64>,
    // Parsed for forward compatibility/documentation. This is a subset of
    // output_tokens and must never be added to the total a second time.
    #[allow(dead_code)]
    #[serde(default, deserialize_with = "optional_token_count")]
    reasoning_output_tokens: Option<i64>,
}

impl TokenUsage {
    fn counts(&self) -> [i64; 5] {
        let input = self.input_tokens.unwrap_or(0).max(0);
        let output = self.output_tokens.unwrap_or(0).max(0);
        [
            input,
            output,
            self.cached_input_tokens.unwrap_or(0).max(0),
            self.cache_write_input_tokens.unwrap_or(0).max(0),
            self.total_tokens.unwrap_or(0).max(input + output),
        ]
    }

    fn from_counts([input, output, cached, written, total]: [i64; 5]) -> Self {
        Self {
            input_tokens: Some(input),
            output_tokens: Some(output),
            cached_input_tokens: Some(cached),
            cache_write_input_tokens: Some(written),
            total_tokens: Some(total),
            reasoning_output_tokens: None,
        }
    }

    fn has_components(&self) -> bool {
        self.input_tokens.unwrap_or(0) > 0 || self.output_tokens.unwrap_or(0) > 0
    }

    fn has_tokens(&self) -> bool {
        self.counts()[4] > 0
    }

    fn same_counts(&self, other: &Self) -> bool {
        self.counts() == other.counts()
    }

    fn plus(&self, other: &Self) -> Self {
        let a = self.counts();
        let b = other.counts();
        Self::from_counts(std::array::from_fn(|i| a[i].saturating_add(b[i])))
    }

    fn minus(&self, other: &Self) -> Self {
        let a = self.counts();
        let b = other.counts();
        Self::from_counts(std::array::from_fn(|i| (a[i] - b[i]).max(0)))
    }
}

/// Tolerate numeric strings and independently missing/malformed components;
/// one changed field must not hide the other counters or the model entirely.
fn optional_token_count<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok())))
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
    fn into_window(self, scope: Option<&str>) -> UsageWindow {
        let util = clamp01(self.used_percent.unwrap_or(0.0) / 100.0);
        let base = window_title(self.window_minutes);
        let title = match scope {
            Some(name) => format!("{base} ({name})"),
            None => base,
        };
        UsageWindow {
            utilization: util,
            remaining: clamp01(1.0 - util),
            resets_at: self.resets_at.and_then(epoch_to_rfc3339),
            title,
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
    use serde_json::{json, Value};

    fn row(kind: &str, payload: Value) -> Value {
        json!({"type":kind,"timestamp":"2026-09-07T02:28:21Z","payload":payload})
    }

    fn counts(input: i64, output: i64) -> Value {
        json!({"input_tokens":input,"output_tokens":output,"total_tokens":input+output})
    }

    fn detail(id: &str, usage: Value) -> Value {
        row(
            "token_usage_record",
            json!({"response_id":id,"usage":usage}),
        )
    }

    fn token_count(total: Value, last: Value) -> Value {
        row(
            "event_msg",
            json!({"type":"token_count","info":{
                "total_token_usage":total,"last_token_usage":last
            }}),
        )
    }

    fn parse(rows: &[Value]) -> Vec<CodexEvent> {
        parse_events(
            &rows
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    fn token_sum(events: &[CodexEvent]) -> i64 {
        events.iter().map(|e| e.usage.counts()[4]).sum()
    }

    #[test]
    fn september_compaction_uses_request_breakdown_and_updated_model() {
        // Sanitized metadata from the affected September 7 rollout. The last
        // turn context was September 1; settings changed without a new context.
        let events = parse(&[
            json!({"type":"turn_context","timestamp":"2026-09-01T09:30:14Z",
                "payload":{"model":"gpt-5.6-sol","cwd":"/work/project"}}),
            json!({"type":"event_msg","timestamp":"2026-09-01T09:31:00Z",
                "payload":{"type":"token_count","info":{
                    "total_token_usage":counts(175022424,524597),
                    "last_token_usage":counts(238266,468)}}}),
            row(
                "event_msg",
                json!({"type":"thread_settings_applied",
                "thread_settings":{"model":"gpt-6-astra"}}),
            ),
            detail(
                "response-1",
                json!({"input_tokens":238841,"cached_input_tokens":11136,
                "cache_write_input_tokens":0,"output_tokens":2634,"total_tokens":241475}),
            ),
            row("compacted", json!({})),
            token_count(
                counts(0, 0),
                json!({"input_tokens":0,"output_tokens":0,"total_tokens":9336}),
            ),
        ]);
        assert_eq!(events.len(), 2);
        let start = parse_ts("2026-09-07T00:00:00Z").unwrap();
        let inside: Vec<_> = events.iter().filter(|e| e.ts >= start).collect();
        assert_eq!(inside.len(), 1);
        let e = inside[0];
        assert_eq!(e.model, "gpt-6-astra");
        assert_eq!(e.project, "project");
        assert_eq!(e.usage.counts(), [238841, 2634, 11136, 0, 241475]);
        let mut t = Totals::default();
        t.add_priced(&e.usage, &e.model, &Pricing::bundled(), e.ts);
        assert!(t.cost().unwrap().total() > 0.0);
        assert_eq!(t.total, t.input + t.output);
    }

    #[test]
    fn mixed_formats_equal_requests_and_quota_repeats_count_once() {
        let events = parse(&[
            row("turn_context", json!({"model":"gpt-5.6-sol"})),
            token_count(counts(100, 10), counts(100, 10)),
            token_count(counts(100, 10), counts(100, 10)),
            row(
                "event_msg",
                json!({"type":"thread_settings_applied",
                "thread_settings":{"model":"gpt-6.1-sol"}}),
            ),
            detail("a", counts(100, 10)),
            token_count(counts(200, 20), counts(100, 10)),
            detail("b", counts(100, 10)),
            token_count(counts(300, 30), counts(100, 10)),
            detail("b", counts(100, 10)), // replay of the same response
            token_count(counts(300, 30), counts(100, 10)),
            row("turn_context", json!({"model":"provider/new-open-model"})),
            token_count(counts(400, 40), counts(100, 10)), // counter-only fallback
            detail("tail", counts(100, 10)),               // active file: no counter written yet
        ]);
        assert_eq!(events.len(), 5);
        assert_eq!(token_sum(&events), 550);
        assert_eq!(
            events.iter().map(|e| e.model.as_str()).collect::<Vec<_>>(),
            [
                "gpt-5.6-sol",
                "gpt-6.1-sol",
                "gpt-6.1-sol",
                "provider/new-open-model",
                "provider/new-open-model"
            ]
        );
    }

    #[test]
    fn compaction_request_is_not_lost_when_legacy_totals_exclude_it() {
        let events = parse(&[
            token_count(counts(100, 10), counts(100, 10)),
            detail("compaction", counts(200, 20)),
            row("compacted", json!({})),
            token_count(counts(100, 10), json!({"total_tokens":90})),
            detail("next", counts(50, 5)),
            token_count(counts(150, 15), counts(50, 5)),
        ]);
        assert_eq!(events.len(), 3);
        assert_eq!(token_sum(&events), 385);
    }

    #[test]
    fn multiple_details_and_counter_only_gaps_are_reconciled() {
        let events = parse(&[
            token_count(counts(100, 10), counts(100, 10)),
            detail("a", counts(50, 5)),
            detail("b", counts(50, 5)),
            token_count(counts(200, 20), counts(50, 5)),
            detail("d", counts(50, 5)),
            detail("e", counts(50, 5)),
            // The delta includes another 50+5 request without a detail.
            token_count(counts(350, 35), counts(50, 5)),
        ]);
        assert_eq!(events.len(), 6);
        assert_eq!(token_sum(&events), 385);
    }

    #[test]
    fn models_are_preserved_without_an_allowlist_or_invented_attribution() {
        let events = parse(&[
            detail("unknown", counts(10, 1)),
            row(
                "turn_context",
                json!({"collaboration_mode":{"settings":{"model":"custom/model"}},
                "cwd":"/work/first"}),
            ),
            detail("custom", counts(10, 1)),
            row(
                "thread_settings_applied",
                json!({"thread_settings":{
                "collaboration_mode":{"settings":{"model":"ATXC44Y9/gpt-6-astra"}}}}),
            ),
            detail("prefixed", counts(10, 1)),
            row(
                "token_usage_record",
                json!({"model":"codex-auto-review","usage":counts(10,1)}),
            ),
            row("turn_context", json!({"model":" ","cwd":"/work/second"})),
            detail("inherited", counts(10, 1)),
        ]);
        assert_eq!(
            events.iter().map(|e| e.model.as_str()).collect::<Vec<_>>(),
            [
                "unknown",
                "custom/model",
                "ATXC44Y9/gpt-6-astra",
                "codex-auto-review",
                "ATXC44Y9/gpt-6-astra"
            ]
        );
        assert_eq!(events[4].project, "second");
        let mut t = Totals::default();
        t.add_priced(&events[3].usage, &events[3].model, &Pricing::bundled(), 0);
        assert!(t.cost().is_none());
    }

    #[test]
    fn malformed_quota_and_partial_lines_do_not_hide_usage() {
        let mut counter = token_count(counts(100, 10), counts(100, 10));
        counter["payload"]["rate_limits"] =
            json!({"primary":"new schema","credits":{"balance":{}}});
        let mut text = counter.to_string();
        text.push_str("\n{broken json}\n");
        text.push_str(
            &detail(
                "numeric",
                json!({"input_tokens":"12","output_tokens":"3",
            "cached_input_tokens":"4","cache_write_input_tokens":{},"reasoning_output_tokens":[]}),
            )
            .to_string(),
        );
        text.push_str("\n{\"type\":\"token_usage_record\"");
        let events = parse_events(&text);
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].usage.counts(), [12, 3, 4, 0, 15]);
        assert_eq!(token_sum(&events), 125);
    }

    #[test]
    fn malformed_detail_leaves_legacy_fallback_available() {
        let mut bad_time = detail("a", counts(100, 10));
        bad_time["timestamp"] = json!("bad timestamp");
        let events = parse(&[
            bad_time,
            detail("b", json!("changed shape")),
            token_count(counts(100, 10), counts(100, 10)),
        ]);
        assert_eq!(events.len(), 1);
        assert_eq!(token_sum(&events), 110);
    }

    #[test]
    fn empty_counter_does_not_reset_the_cumulative_baseline() {
        let events = parse(&[
            token_count(counts(100, 10), counts(100, 10)),
            token_count(json!({}), json!({})),
            token_count(counts(200, 20), counts(100, 10)),
        ]);
        assert_eq!(token_sum(&events), 220);
    }

    #[test]
    fn real_total_only_usage_is_retained_without_a_fabricated_price() {
        let events = parse(&[
            row("turn_context", json!({"model":"gpt-6-astra"})),
            token_count(json!({"total_tokens":8033}), json!({"total_tokens":8033})),
        ]);
        assert_eq!(events.len(), 1);
        let mut total = Totals::default();
        total.add_priced(&events[0].usage, &events[0].model, &Pricing::bundled(), 0);
        assert_eq!(total.total, 8033);
        assert!(total.cost().is_none());
    }

    /// Read-only diagnostic: no transcript contents, account identifiers, or
    /// database writes. Checks request preservation against actual local logs.
    #[test]
    #[ignore = "reads local Codex rollouts"]
    fn local_rollout_parser_audit() {
        let files = session_files();
        let mut details = 0;
        let mut legacy_tokens = 0;
        let mut inventory: std::collections::BTreeMap<String, Totals> = Default::default();
        let mut token_only: std::collections::BTreeMap<String, i64> = Default::default();
        let pricing = Pricing::bundled();
        for file in &files {
            let text = std::fs::read_to_string(file).unwrap();
            let events = parse_events(&text);
            let actual: HashMap<_, _> = events
                .iter()
                .filter_map(|e| {
                    e.request_id
                        .as_ref()
                        .map(|id| (id.1.clone(), e.usage.counts()))
                })
                .collect();
            let mut expected = HashMap::new();
            let mut counter = UsageCounter::default();
            for line in text.lines() {
                let Ok(r) = serde_json::from_str::<UsageLogRow>(line) else {
                    continue;
                };
                if r.r#type == "token_usage_record"
                    && r.timestamp.as_deref().and_then(parse_ts).is_some()
                {
                    if let (Some(id), Some(u)) = (
                        nonempty_string(r.payload.get("response_id")),
                        r.payload.get("usage").and_then(decode_usage),
                    ) {
                        if u.has_tokens() {
                            expected.entry(id.to_string()).or_insert(u.counts());
                        }
                    }
                }
                if r.payload.get("type").and_then(Value::as_str) == Some("token_count") {
                    if let Some(v) = r.payload.get("info") {
                        let i = Info {
                            total_token_usage: v.get("total_token_usage").and_then(decode_usage),
                            last_token_usage: v.get("last_token_usage").and_then(decode_usage),
                        };
                        if let Some(delta) = counter.delta(&i) {
                            legacy_tokens += delta.counts()[4];
                        }
                    }
                }
            }
            assert_eq!(actual, expected, "detailed request coverage differs");
            details += actual.len();
            for e in events {
                if e.usage.counts()[4] > e.usage.counts()[0] + e.usage.counts()[1] {
                    let day = DateTime::from_timestamp(e.ts, 0)
                        .unwrap()
                        .format("%Y-%m-%d")
                        .to_string();
                    *token_only.entry(day).or_default() += 1;
                }
                inventory
                    .entry(e.model.clone())
                    .or_default()
                    .add_priced(&e.usage, &e.model, &pricing, e.ts);
            }
        }
        let parsed_tokens: i64 = inventory.values().map(|t| t.total).sum();
        println!("files={} unique_detailed_requests={} legacy_tokens={} parsed_tokens={} recovered_tokens={}",
            files.len(),details,legacy_tokens,parsed_tokens,parsed_tokens-legacy_tokens);
        for (model, t) in inventory {
            println!(
                "model={model} tokens={} token_only={} cost={:?}",
                t.total,
                t.total - t.input - t.output,
                t.cost().map(|c| c.total())
            );
        }
        println!("token_only_events_by_date={token_only:?}");

        // Exclude the active day so concurrent appends cannot make the two
        // independent scans disagree. Team sync consumes the calendar rows.
        let end = Utc::now().date_naive();
        let start = end - chrono::Duration::days(90);
        let local = collect_local_range(
            start.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp(),
            end.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp(),
        );
        let daily = collect_rows(
            &start.to_string(),
            &(end - chrono::Duration::days(1)).to_string(),
        );
        let mut by_model: HashMap<String, ([i64; 6], f64)> = HashMap::new();
        for r in daily {
            let entry = by_model.entry(r.model).or_default();
            for (sum, value) in entry.0.iter_mut().zip([
                r.input,
                r.output,
                r.cache_read,
                r.cache_create,
                r.unattributed,
                r.tokens,
            ]) {
                *sum += value;
            }
            entry.1 += r.cost;
        }
        assert_eq!(local.len(), by_model.len());
        for m in &local {
            let (counts, cost) = &by_model[&m.id];
            assert_eq!(
                *counts,
                [
                    m.input,
                    m.output,
                    m.cache_read,
                    m.cache_create,
                    m.unattributed,
                    m.total
                ]
            );
            if let Some(c) = &m.cost {
                assert!((c.total - cost).abs() < 1e-6);
            }
        }
        println!(
            "local/calendar/team component and price agreement: {} models, 90 completed days",
            local.len()
        );
    }

    #[test]
    fn copied_detailed_requests_are_deduplicated_across_files() {
        let dir = std::env::temp_dir().join(format!(
            "hpbar-codex-parser-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let files = [dir.join("one.jsonl"), dir.join("two.jsonl")];
        let text = [
            detail("shared", counts(100, 10)),
            token_count(counts(100, 10), counts(100, 10)),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
        for file in &files {
            std::fs::write(file, &text).unwrap();
        }
        let mut partial = text.as_bytes().to_vec();
        partial.extend_from_slice(b"\n{\"type\":\"");
        partial.push(0xf0); // incomplete UTF-8 character in an active append
        std::fs::write(&files[1], partial).unwrap();
        let events = collect_events(&files);
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(token_sum(&events), 110);
    }

    fn info(input: i64, output: i64) -> Info {
        Info {
            total_token_usage: Some(TokenUsage {
                input_tokens: Some(input),
                output_tokens: Some(output),
                total_tokens: Some(input + output),
                ..Default::default()
            }),
            last_token_usage: Some(TokenUsage {
                input_tokens: Some(100),
                output_tokens: Some(10),
                total_tokens: Some(110),
                ..Default::default()
            }),
        }
    }

    #[test]
    fn repeated_quota_events_are_not_new_turns() {
        let mut counter = UsageCounter::default();
        assert_eq!(
            counter.delta(&info(100, 10)).unwrap().total_tokens,
            Some(110)
        );
        assert!(counter.delta(&info(100, 10)).is_none());
        // Equal-sized genuine turns still count.
        assert_eq!(
            counter.delta(&info(200, 20)).unwrap().total_tokens,
            Some(110)
        );
        assert!(counter.delta(&info(200, 20)).is_none());
    }

    #[test]
    fn counters_before_range_and_compaction_are_handled() {
        let mut counter = UsageCounter::default();
        counter.delta(&info(1000, 100));
        let inside = counter.delta(&info(1100, 110)).unwrap();
        assert_eq!(inside.input_tokens, Some(100));
        assert_eq!(
            counter.delta(&info(100, 10)).unwrap().total_tokens,
            Some(110)
        );
        assert!(counter.delta(&info(100, 10)).is_none());
    }

    #[test]
    fn cache_writes_are_priced_once_as_part_of_input() {
        let mut total = Totals::default();
        total.add_priced(
            &TokenUsage {
                input_tokens: Some(100_000),
                cached_input_tokens: Some(20_000),
                cache_write_input_tokens: Some(30_000),
                output_tokens: Some(10_000),
                ..Default::default()
            },
            "gpt-6.1-sol",
            &Pricing::bundled(),
            0,
        );
        assert_eq!(total.total, 110_000);
        // 50K ordinary input + 20K read + 30K write + 10K output.
        assert!((total.cost().unwrap().total() - 0.277).abs() < 1e-9);
    }

    #[test]
    fn reasoning_is_not_added_twice() {
        let mut totals = Totals::default();
        totals.add(&TokenUsage {
            cache_write_input_tokens: None,
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

    #[test]
    fn scoped_fallback_labels_the_bar_instead_of_a_detail_box() {
        let limits: RateLimits = serde_json::from_str(
            r#"{
              "primary":{"used_percent":0,"window_minutes":10080,"resets_at":1786000000},
              "secondary":null,
              "plan_type":"prolite",
              "limit_name":"GPT-5.3-Codex-Spark",
              "credits":{"has_credits":false,"unlimited":false,"balance":"0"}
            }"#,
        )
        .unwrap();
        let report = build_quota(limits).unwrap();
        assert_eq!(report.windows[0].title, "Weekly (GPT-5.3-Codex-Spark)");
        assert!(report.details.is_empty());
    }
}
