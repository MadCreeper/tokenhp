//! Per-model token usage from Claude Code's local session transcripts
//! (`~/.claude/projects/**/*.jsonl`). Port of the Swift `LocalStatsDataSource`.
//!
//! Each assistant message carries `message.model` and a `message.usage`
//! breakdown (input / cache_creation / cache_read / output). We aggregate every
//! model id seen within the time window and price each via the bundled table.

use crate::pricing::Pricing;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const ASSISTANT_MARKER: &str = "\"type\":\"assistant\"";

#[derive(Serialize, Clone)]
pub struct ModelCostDTO {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_create: f64,
    pub total: f64,
}

#[derive(Serialize, Clone)]
pub struct ModelUsageDTO {
    pub id: String,
    pub display_name: String,
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_create: i64,
    pub total: i64,
    /// Largest single bucket; breakdown bars fill proportionally to this.
    pub max_component: i64,
    pub cost: Option<ModelCostDTO>,
}

/// Per-model Claude Code token usage over the last `window_secs`, from
/// `~/.claude/projects`. Returns an empty vec when there's nothing; this is the
/// Claude Code [`crate::tools::ToolAdapter`] implementation's data source.
/// Synchronous (file IO) — call via `spawn_blocking`.
pub fn collect(window_secs: i64) -> Vec<ModelUsageDTO> {
    let projects = dirs::home_dir()
        .map(|h| h.join(".claude").join("projects"))
        .unwrap_or_default();

    let files = session_files(&projects);
    let now = Utc::now().timestamp();
    let mut totals: HashMap<String, Totals> = HashMap::new();

    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for line in text.lines() {
            // Cheap substring check skips the ~half of lines that aren't
            // assistant messages before we pay for JSON parsing.
            if !line.contains(ASSISTANT_MARKER) {
                continue;
            }
            let Ok(row) = serde_json::from_str::<Row>(line) else {
                continue;
            };
            if row.r#type.as_deref() != Some("assistant") {
                continue;
            }
            let Some(ts) = row.timestamp.as_deref().and_then(parse_ts) else {
                continue;
            };
            let age = now - ts;
            if age < 0 || age >= window_secs {
                continue;
            }
            let Some(msg) = row.message else { continue };
            let Some(model) = msg.model else { continue };
            if model.starts_with('<') {
                continue; // skip placeholder ids like "<synthetic>"
            }
            totals.entry(model).or_default().add(msg.usage.as_ref());
        }
    }

    let pricing = Pricing::loaded();
    let mut models: Vec<ModelUsageDTO> = totals
        .into_iter()
        .map(|(id, t)| {
            let cache_create = t.cache_create_5m + t.cache_create_1h;
            let cost = pricing
                .cost(
                    &id,
                    t.input,
                    t.output,
                    t.cache_read,
                    t.cache_create_5m,
                    t.cache_create_1h,
                )
                .map(|c| ModelCostDTO {
                    input: c.input,
                    output: c.output,
                    cache_read: c.cache_read,
                    cache_create: c.cache_create,
                    total: c.total(),
                });
            let total = t.input + t.output + t.cache_read + cache_create;
            let max_component = t.input.max(t.output).max(t.cache_read).max(cache_create);
            ModelUsageDTO {
                display_name: display_name(&id),
                id,
                input: t.input,
                output: t.output,
                cache_read: t.cache_read,
                cache_create,
                total,
                max_component,
                cost,
            }
        })
        .collect();

    models.sort_by(|a, b| b.total.cmp(&a.total));
    models
}

#[derive(Default)]
struct Totals {
    input: i64,
    output: i64,
    cache_read: i64,
    cache_create_5m: i64,
    cache_create_1h: i64,
}

impl Totals {
    fn add(&mut self, u: Option<&Usage>) {
        let Some(u) = u else { return };
        self.input += u.input_tokens.unwrap_or(0);
        self.output += u.output_tokens.unwrap_or(0);
        self.cache_read += u.cache_read_input_tokens.unwrap_or(0);
        let cc5 = u
            .cache_creation
            .as_ref()
            .and_then(|c| c.ephemeral_5m_input_tokens)
            .unwrap_or(0);
        let cc1 = u
            .cache_creation
            .as_ref()
            .and_then(|c| c.ephemeral_1h_input_tokens)
            .unwrap_or(0);
        let cct = u.cache_creation_input_tokens.unwrap_or(0);
        // Older rows may omit the breakdown but populate the total; default
        // those to the cheaper 5m bucket.
        if cc5 + cc1 == 0 && cct > 0 {
            self.cache_create_5m += cct;
        } else {
            self.cache_create_5m += cc5;
            self.cache_create_1h += cc1;
        }
    }
}

#[derive(Deserialize)]
struct Row {
    r#type: Option<String>,
    timestamp: Option<String>,
    /// Working directory the session ran in, e.g. "/Users/me/projects/app".
    /// Present on assistant rows; its final component is the project name.
    cwd: Option<String>,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    model: Option<String>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: Option<i64>,
    cache_creation_input_tokens: Option<i64>,
    cache_read_input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_creation: Option<CacheCreation>,
}

#[derive(Deserialize)]
struct CacheCreation {
    ephemeral_5m_input_tokens: Option<i64>,
    ephemeral_1h_input_tokens: Option<i64>,
}

/// Recursively collect every `*.jsonl` under `dir`.
fn session_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_jsonl(dir, &mut out);
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

// ----------------------------------------------------------------- per-day
// The Team feature needs exact per-calendar-day history (not the rolling windows
// above) so daily/weekly/monthly totals sum without overlap. We emit the most
// granular unit — (day, project, model) — which the team DB stores directly.

/// One (UTC day, project, model) usage bucket. `tokens` is the sum of the four
/// components; `cost` is priced via the bundled table.
#[derive(Clone)]
pub struct UsageRow {
    pub day: String, // YYYY-MM-DD (UTC)
    pub project: String,
    pub model: String,
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_create: i64,
    pub tokens: i64,
    pub cost: f64,
    /// Unix seconds of the latest activity in this (day, project) — drives
    /// "current project" and member staleness.
    pub last_active: i64,
}

#[derive(Default)]
struct ProjAcc {
    models: HashMap<String, Totals>,
    last_active: i64,
}

/// Exact per-(calendar-day, project, model) Claude Code usage over the inclusive
/// day range [`start_day`, `end_day`] (both "YYYY-MM-DD", UTC). Computed from
/// message timestamps, so daily/weekly/monthly sums never overlap. Synchronous
/// (file IO) — call via `spawn_blocking`.
pub fn collect_rows(start_day: &str, end_day: &str) -> Vec<UsageRow> {
    let projects_dir = dirs::home_dir()
        .map(|h| h.join(".claude").join("projects"))
        .unwrap_or_default();
    let files = session_files(&projects_dir);

    // (day, project) -> per-model token buckets + latest activity.
    let mut acc: HashMap<(String, String), ProjAcc> = HashMap::new();

    for file in files {
        let fallback = encoded_dir_name(&file);
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for line in text.lines() {
            if !line.contains(ASSISTANT_MARKER) {
                continue;
            }
            let Ok(row) = serde_json::from_str::<Row>(line) else {
                continue;
            };
            if row.r#type.as_deref() != Some("assistant") {
                continue;
            }
            let Some(ts) = row.timestamp.as_deref().and_then(parse_ts) else {
                continue;
            };
            let Some(day) = day_string(ts) else { continue };
            if day.as_str() < start_day || day.as_str() > end_day {
                continue;
            }
            let project = row
                .cwd
                .as_deref()
                .map(project_name)
                .unwrap_or_else(|| fallback.clone());
            let Some(msg) = row.message else { continue };
            let Some(model) = msg.model else { continue };
            if model.starts_with('<') {
                continue;
            }
            let e = acc.entry((day, project)).or_default();
            e.models.entry(model).or_default().add(msg.usage.as_ref());
            if ts > e.last_active {
                e.last_active = ts;
            }
        }
    }

    let pricing = Pricing::loaded();
    let mut rows: Vec<UsageRow> = Vec::new();
    for ((day, project), pa) in acc {
        let last_active = pa.last_active;
        for (model, t) in pa.models {
            let cache_create = t.cache_create_5m + t.cache_create_1h;
            let tokens = t.input + t.output + t.cache_read + cache_create;
            let cost = pricing
                .cost(
                    &model,
                    t.input,
                    t.output,
                    t.cache_read,
                    t.cache_create_5m,
                    t.cache_create_1h,
                )
                .map(|c| c.total())
                .unwrap_or(0.0);
            rows.push(UsageRow {
                day: day.clone(),
                project: project.clone(),
                model,
                input: t.input,
                output: t.output,
                cache_read: t.cache_read,
                cache_create,
                tokens,
                cost,
                last_active,
            });
        }
    }
    rows.sort_by(|a, b| a.day.cmp(&b.day).then(b.tokens.cmp(&a.tokens)));
    rows
}

/// Final path component of a working directory: "/Users/me/projects/app" →
/// "app". The label shown for a project.
pub fn project_name(cwd: &str) -> String {
    let trimmed = cwd.trim_end_matches(['/', '\\']);
    let name = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
    if name.is_empty() {
        "unknown".to_string()
    } else {
        name.to_string()
    }
}

/// "YYYY-MM-DD" (UTC) for a unix timestamp.
fn day_string(ts: i64) -> Option<String> {
    DateTime::<Utc>::from_timestamp(ts, 0).map(|dt| dt.format("%Y-%m-%d").to_string())
}

/// Best-effort project label from the encoded `~/.claude/projects/<dir>` name,
/// used only when a row lacks `cwd`. The dir encodes the cwd with separators
/// replaced by '-', so we take the trailing segment.
fn encoded_dir_name(file: &Path) -> String {
    file.parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|s| s.trim_start_matches('-').rsplit('-').next().unwrap_or(s).to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

pub fn window_label(window_secs: i64) -> &'static str {
    match window_secs {
        86_400 => "last 24h",
        604_800 => "last 7 days",
        2_592_000 => "last 30 days",
        _ => "recent",
    }
}

/// `claude-opus-4-8` → "Opus 4.8", `claude-3-5-sonnet-20240620` → "Sonnet 3.5".
/// Non-Claude ids pass through verbatim. Port of the Swift `displayName`.
pub fn display_name(id: &str) -> String {
    if !id.to_lowercase().starts_with("claude") {
        return id.to_string();
    }
    let mut parts: Vec<String> = id.split('-').map(|s| s.to_string()).collect();
    // Drop a trailing date stamp like "20251001".
    while let Some(last) = parts.last() {
        if last.len() >= 6 && last.chars().all(|c| c.is_ascii_digit()) {
            parts.pop();
        } else {
            break;
        }
    }
    if parts.first().map(|s| s.to_lowercase()) == Some("claude".to_string()) {
        parts.remove(0);
    }

    let families = ["opus", "sonnet", "haiku"];
    let Some(fam_idx) = parts
        .iter()
        .position(|p| families.contains(&p.to_lowercase().as_str()))
    else {
        return id.to_string();
    };
    let family = capitalize(&parts[fam_idx]);
    let version: Vec<String> = parts
        .iter()
        .enumerate()
        .filter(|(i, p)| *i != fam_idx && !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        .map(|(_, p)| p.clone())
        .collect();
    if version.is_empty() {
        family
    } else {
        format!("{} {}", family, version.join("."))
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}
