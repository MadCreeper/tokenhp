//! The "API" usage axis: every local AI CLI is a [`ToolAdapter`] that scans its
//! own logs for per-model token usage. We surface a per-tool breakdown (`apps`)
//! plus a pooled-by-model total (`combined`), so the UI can show "all tools at
//! once" or drill into one. Adding a tool = one more adapter in [`adapters`].
//!
//! Each tool is tagged `equivalent` (a flat-rate subscription, priced at API
//! rates only for comparison — Claude Code, Codex) or `real` (actual metered
//! API-key spend). This mirrors the Subscription axis (live quota per provider)
//! on the other side of the UI.

use crate::codexstats;
use crate::localstats::{self, window_label, ModelCostDTO, ModelUsageDTO};
use crate::openclawstats;
use serde::Serialize;
use std::collections::HashMap;

/// One tool's usage over the window.
#[derive(Serialize, Clone)]
pub struct AppUsageDTO {
    pub id: String,
    pub display_name: String,
    /// "equivalent" (subscription, priced at API rates) or "real" (API spend).
    pub kind: String,
    pub models: Vec<ModelUsageDTO>,
    pub total: i64,
    pub cost: Option<f64>,
}

/// The API view: per-tool breakdown plus a model-pooled total across all tools.
#[derive(Serialize, Clone)]
pub struct LocalReport {
    pub apps: Vec<AppUsageDTO>,
    pub combined: Vec<ModelUsageDTO>,
    pub source_label: String,
}

/// A local tool whose logs we can scan for per-model token usage.
pub trait ToolAdapter {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn kind(&self) -> &'static str;
    fn collect(&self, window_secs: i64) -> Vec<ModelUsageDTO>;
}

struct ClaudeCode;
impl ToolAdapter for ClaudeCode {
    fn id(&self) -> &'static str {
        "claude-code"
    }
    fn display_name(&self) -> &'static str {
        "Claude Code"
    }
    fn kind(&self) -> &'static str {
        "equivalent"
    }
    fn collect(&self, window_secs: i64) -> Vec<ModelUsageDTO> {
        localstats::collect(window_secs)
    }
}

struct Codex;
impl ToolAdapter for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }
    fn display_name(&self) -> &'static str {
        "Codex"
    }
    fn kind(&self) -> &'static str {
        "equivalent"
    }
    fn collect(&self, window_secs: i64) -> Vec<ModelUsageDTO> {
        codexstats::collect_local(window_secs)
    }
}

struct OpenClaw;
impl ToolAdapter for OpenClaw {
    fn id(&self) -> &'static str {
        "openclaw"
    }
    fn display_name(&self) -> &'static str {
        "OpenClaw"
    }
    fn kind(&self) -> &'static str {
        "real" // drives real API keys (DeepSeek/LiteLLM/…), so this is metered spend
    }
    fn collect(&self, window_secs: i64) -> Vec<ModelUsageDTO> {
        openclawstats::collect_local(window_secs)
    }
}

/// The registry. Add a tool here and it shows up on the API axis automatically.
fn adapters() -> Vec<Box<dyn ToolAdapter>> {
    vec![Box::new(ClaudeCode), Box::new(Codex), Box::new(OpenClaw)]
}

/// Aggregate every tool's usage over `window_secs` into per-tool + pooled views.
pub fn fetch_local(window_secs: i64) -> Result<LocalReport, String> {
    let mut apps: Vec<AppUsageDTO> = Vec::new();
    let mut pool: HashMap<String, ModelUsageDTO> = HashMap::new();

    for adapter in adapters() {
        let models = adapter.collect(window_secs);
        if models.is_empty() {
            continue;
        }
        for m in &models {
            merge_into(&mut pool, m);
        }
        apps.push(AppUsageDTO {
            id: adapter.id().into(),
            display_name: adapter.display_name().into(),
            kind: adapter.kind().into(),
            total: models.iter().map(|m| m.total).sum(),
            cost: sum_costs(&models),
            models,
        });
    }

    if apps.is_empty() {
        return Err("No local tool activity in this window.".into());
    }

    let mut combined: Vec<ModelUsageDTO> = pool.into_values().collect();
    combined.sort_by(|a, b| b.total.cmp(&a.total));

    Ok(LocalReport {
        apps,
        combined,
        source_label: format!("Local API usage · {}", window_label(window_secs)),
    })
}

/// Total $ across a tool's models, or `None` if nothing was priced.
fn sum_costs(models: &[ModelUsageDTO]) -> Option<f64> {
    let mut sum = 0.0;
    let mut any = false;
    for m in models {
        if let Some(c) = &m.cost {
            any = true;
            sum += c.total;
        }
    }
    any.then_some(sum)
}

/// Fold a model's usage into the pooled-by-id total (summing across tools).
fn merge_into(pool: &mut HashMap<String, ModelUsageDTO>, m: &ModelUsageDTO) {
    let e = pool.entry(m.id.clone()).or_insert_with(|| ModelUsageDTO {
        id: m.id.clone(),
        display_name: m.display_name.clone(),
        input: 0,
        output: 0,
        cache_read: 0,
        cache_create: 0,
        total: 0,
        max_component: 0,
        cost: None,
    });
    e.input += m.input;
    e.output += m.output;
    e.cache_read += m.cache_read;
    e.cache_create += m.cache_create;
    e.total = e.input + e.output + e.cache_read + e.cache_create;
    e.max_component = e.input.max(e.output).max(e.cache_read).max(e.cache_create);
    if let Some(c) = &m.cost {
        let acc = e.cost.get_or_insert(ModelCostDTO {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_create: 0.0,
            total: 0.0,
        });
        acc.input += c.input;
        acc.output += c.output;
        acc.cache_read += c.cache_read;
        acc.cache_create += c.cache_create;
        acc.total += c.total;
    }
}
