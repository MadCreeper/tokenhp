//! OpenClaw local token usage — the `real` API-spend [`crate::tools::ToolAdapter`].
//!
//! OpenClaw writes per-turn trajectory event logs at
//! `~/.openclaw/agents/<agent>/sessions/*.trajectory.jsonl`. Each `model.completed`
//! event carries a per-turn `data.usage` block (input / output / cacheRead) plus
//! `provider` / `modelId`. We sum those per model within the window. OpenClaw
//! drives real API keys (DeepSeek, LiteLLM, …), so this is metered spend priced
//! at the bundled API rates — hence the tool is tagged `kind = "real"`.

use crate::localstats::{ModelCostDTO, ModelUsageDTO};
use crate::pricing::Pricing;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const MODEL_COMPLETED_MARKER: &str = "\"type\":\"model.completed\"";

/// Per-model OpenClaw token usage over the last `window_secs`, from
/// `~/.openclaw/agents`. Empty vec when there's nothing. Synchronous file IO —
/// the [`crate::tools`] aggregator already runs adapters on a blocking thread.
pub fn collect_local(window_secs: i64) -> Vec<ModelUsageDTO> {
    let root = dirs::home_dir()
        .map(|h| h.join(".openclaw").join("agents"))
        .unwrap_or_default();

    let mut files = Vec::new();
    collect_trajectories(&root, &mut files);

    let now = Utc::now().timestamp();
    let mut totals: HashMap<String, Totals> = HashMap::new();

    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
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
            let age = now - ts;
            if age < 0 || age >= window_secs {
                continue;
            }
            let Some(model) = ev.model_id else { continue };
            // `data.usage` is the per-turn total (NOT cumulative); sum it.
            let Some(usage) = ev.data.and_then(|d| d.usage) else {
                continue;
            };
            totals.entry(model).or_default().add(&usage);
        }
    }

    let pricing = Pricing::loaded();
    let mut models: Vec<ModelUsageDTO> = totals
        .into_iter()
        .map(|(id, t)| {
            // OpenClaw exposes no per-turn cache-write figure → cache_create = 0.
            let cost = pricing
                .cost(&id, t.input, t.output, t.cache_read, 0, 0)
                .map(|c| ModelCostDTO {
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

#[derive(Default)]
struct Totals {
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
    DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.timestamp())
}
