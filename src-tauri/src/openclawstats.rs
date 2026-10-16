//! OpenClaw local token usage — the `real` API-spend [`crate::tools::ToolAdapter`].
//!
//! OpenClaw writes per-turn trajectory event logs at
//! `~/.openclaw/agents/<agent>/sessions/*.trajectory.jsonl`. Each `model.completed`
//! event carries a per-turn `data.usage` block (input / output / cacheRead) plus
//! `provider` / `modelId`. We sum those per model within the window. OpenClaw
//! drives real API keys (DeepSeek, LiteLLM, …), so this is metered spend priced
//! at the bundled API rates — hence the tool is tagged `kind = "real"`.

use crate::localstats::{ModelCostDTO, ModelUsageDTO};
use crate::pricing::{ModelCost, Pricing};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const MODEL_COMPLETED_MARKER: &str = "model.completed";

/// Per-model OpenClaw token usage over the last `window_secs`, from
/// `~/.openclaw/agents`. Empty vec when there's nothing. Synchronous file IO —
/// the [`crate::tools`] aggregator already runs adapters on a blocking thread.
pub fn collect_local(window_secs: i64) -> Vec<ModelUsageDTO> {
    let now = Utc::now().timestamp();
    let totals = scan(now - window_secs + 1, now + 1, false);
    let mut models: Vec<ModelUsageDTO> = totals
        .into_iter()
        .map(|((_, id), t)| {
            // OpenClaw exposes no per-turn cache-write figure → cache_create = 0.
            let cost = (!t.unpriced).then_some(t.priced).map(|c| ModelCostDTO {
                input: c.input,
                output: c.output,
                cache_read: c.cache_read,
                cache_create: c.cache_create,
                total: c.total(),
            });
            let total = t.input + t.output + t.cache_read;
            let max_component = t.input.max(t.output).max(t.cache_read);
            ModelUsageDTO {
                // Non-Claude ids (e.g. "deepseek-v4-pro") render verbatim.
                display_name: id.clone(),
                id,
                input: t.input,
                output: t.output,
                cache_read: t.cache_read,
                cache_create: 0,
                unattributed: 0,
                total,
                max_component,
                cost,
            }
        })
        .filter(|m| m.total > 0)
        .collect();

    models.sort_by(|a, b| b.total.cmp(&a.total));
    models
}

fn scan(start: i64, end: i64, daily: bool) -> HashMap<(String, String), Totals> {
    let root = dirs::home_dir()
        .map(|h| h.join(".openclaw").join("agents"))
        .unwrap_or_default();

    let mut files = Vec::new();
    collect_trajectories(&root, &mut files);

    scan_files(&files, start, end, daily, &Pricing::loaded())
}

fn scan_files(
    files: &[PathBuf],
    start: i64,
    end: i64,
    daily: bool,
    pricing: &Pricing,
) -> HashMap<(String, String), Totals> {
    let mut totals: HashMap<(String, String), Totals> = HashMap::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        for line in text.lines() {
            // Cheap substring gate before paying for JSON parsing.
            if !line.contains(MODEL_COMPLETED_MARKER) {
                continue;
            }
            let Ok(ev) = serde_json::from_str::<Event>(line) else {
                continue;
            };
            if ev.r#type.as_deref() != Some("model.completed") {
                continue;
            }
            let Some(ts) = ev.ts.as_deref().and_then(parse_ts) else {
                continue;
            };
            if ts < start || ts >= end {
                continue;
            }
            let Some(model) = ev.model_id else { continue };
            // `data.usage` is the per-turn total (NOT cumulative); sum it.
            let Some(usage) = ev.data.and_then(|d| d.usage) else {
                continue;
            };
            let day = if daily {
                DateTime::<Utc>::from_timestamp(ts, 0)
                    .unwrap()
                    .format("%Y-%m-%d")
                    .to_string()
            } else {
                String::new()
            };
            let entry = totals.entry((day, model.clone())).or_default();
            entry.add(&usage);
            match pricing.cost_request(
                &model,
                [
                    usage.input.unwrap_or(0),
                    usage.output.unwrap_or(0),
                    usage.cache_read.unwrap_or(0),
                    0,
                    0,
                ],
                ts,
            ) {
                Some(cost) => entry.priced.add(cost),
                None => entry.unpriced = true,
            }
        }
    }

    totals
}

pub fn collect_calendar(start: &str, end: &str) -> Vec<crate::calendar::CalendarEntry> {
    let bounds = |day: &str| {
        chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|d| d.and_utc().timestamp())
    };
    let (Some(start), Some(end)) = (bounds(start), bounds(end)) else {
        return Vec::new();
    };
    scan(start, (end + 86400).min(Utc::now().timestamp() + 1), true)
        .into_iter()
        .map(|((day, model), t)| crate::calendar::CalendarEntry {
            day,
            display_name: model.clone(),
            model,
            tool: "openclaw".into(),
            member: String::new(),
            member_id: String::new(),
            account_key: String::new(),
            account_label: String::new(),
            tokens: t.input + t.output + t.cache_read,
            cost: (!t.unpriced || t.priced.total() > 0.0).then_some(t.priced.total()),
            cost_complete: !t.unpriced,
            cost_estimated: false,
        })
        .collect()
}

#[derive(Default)]
struct Totals {
    priced: ModelCost,
    unpriced: bool,
    input: i64,
    output: i64,
    cache_read: i64,
}

impl Totals {
    fn add(&mut self, u: &Usage) {
        self.input += u.input.unwrap_or(0);
        self.output += u.output.unwrap_or(0);
        self.cache_read += u.cache_read.unwrap_or(0);
    }
}

// --- trajectory JSON (only the fields we read; everything else ignored) ---

#[derive(Deserialize)]
struct Event {
    r#type: Option<String>,
    ts: Option<String>,
    #[serde(rename = "modelId")]
    model_id: Option<String>,
    data: Option<Data>,
}

#[derive(Deserialize)]
struct Data {
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    input: Option<i64>,
    output: Option<i64>,
    #[serde(rename = "cacheRead")]
    cache_read: Option<i64>,
}

/// Collect every `*.trajectory.jsonl` under `dir`. Skips the sibling `.jsonl`,
/// `.jsonl.reset.*` and `.trajectory-path.json` files so turns aren't
/// double-counted from rotated logs.
fn collect_trajectories(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_trajectories(&path, out);
        } else if path
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|n| n.ends_with(".trajectory.jsonl"))
        {
            out.push(path);
        }
    }
}

fn parse_ts(s: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn daily_and_rolling_scans_share_prices_and_utc_boundaries() {
        let file =
            std::env::temp_dir().join(format!("hpbar-calendar-{}.jsonl", uuid::Uuid::new_v4()));
        let event = |ts: &str, model: &str| {
            serde_json::json!({
                "type": "model.completed", "ts": ts, "modelId": model,
                "data": { "usage": { "input": 100, "output": 20, "cacheRead": 30 } }
            })
            .to_string()
        };
        std::fs::write(
            &file,
            [
                event("2023-12-31T23:59:59Z", "gpt-6.1-sol"),
                event("2024-02-29T00:00:00Z", "gpt-6.1-sol"),
                event("2024-03-01T00:30:00+01:00", "gpt-6.1-sol"),
                event("2024-03-01T00:00:00Z", "unknown-model"),
                event("2025-01-01T00:00:00Z", "gpt-6.1-sol"),
            ]
            .join("\n"),
        )
        .unwrap();
        let pricing = Pricing::bundled();
        let start = parse_ts("2024-01-01T00:00:00Z").unwrap();
        let end = parse_ts("2025-01-01T00:00:00Z").unwrap();
        let daily = scan_files(&[file.clone()], start, end, true, &pricing);
        let rolling = scan_files(&[file.clone()], start, end, false, &pricing);
        std::fs::remove_file(file).unwrap();
        assert_eq!(daily.len(), 2);
        let leap = &daily[&("2024-02-29".into(), "gpt-6.1-sol".into())];
        assert_eq!(leap.input, 200);
        assert!(!leap.unpriced);
        assert!(daily[&("2024-03-01".into(), "unknown-model".into())].unpriced);
        assert_eq!(
            daily.values().map(|t| t.input).sum::<i64>(),
            rolling.values().map(|t| t.input).sum::<i64>()
        );
        assert_eq!(
            leap.priced.total(),
            rolling[&("".into(), "gpt-6.1-sol".into())].priced.total()
        );
    }
}
