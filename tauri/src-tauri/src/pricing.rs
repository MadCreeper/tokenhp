//! Per-model token pricing. Port of the Swift `Pricing` / `ModelPrice`.
//!
//! Prices live in JSON, not code. Two sources, merged:
//!   1. Bundled `pricing.json` (compiled in via `include_str!`) — defaults.
//!   2. User overrides at the platform data dir (e.g. macOS
//!      `~/Library/Application Support/HPBar/pricing.json`). Anything here wins.
//!
//! All values are dollars per million tokens.

use serde::Deserialize;
use std::collections::HashMap;

const BUNDLED: &str = include_str!("pricing.json");

#[derive(Deserialize, Clone, Debug)]
pub struct ModelPrice {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_create: f64,
    /// 1-hour-TTL cache writes (2× input for Anthropic). Falls back to
    /// `cache_create` when absent.
    pub cache_create_1h: Option<f64>,
}

/// Per-component dollar cost for a model's usage.
#[derive(Clone, Debug, Default)]
pub struct ModelCost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_create: f64, // combined 5m + 1h
}

impl ModelCost {
    pub fn total(&self) -> f64 {
        self.input + self.output + self.cache_read + self.cache_create
    }
}

pub struct Pricing {
    table: HashMap<String, ModelPrice>,
}

impl Pricing {
    /// Bundled table overlaid with user overrides. Malformed/missing files are
    /// tolerated (bundled wins nothing → empty; user missing → just bundled).
    pub fn loaded() -> Self {
        let mut table: HashMap<String, ModelPrice> =
            serde_json::from_str(BUNDLED).unwrap_or_default();
        for (k, v) in user_overrides() {
            table.insert(k, v);
        }
        Pricing { table }
    }

    fn price(&self, model_id: &str) -> Option<&ModelPrice> {
        self.table
            .get(model_id)
            .or_else(|| self.table.get(&strip_date_suffix(model_id)))
    }

    /// Dollar cost split by component. Cache writes come as two buckets because
    /// 5m and 1h writes bill at different rates.
    pub fn cost(
        &self,
        model_id: &str,
        input: i64,
        output: i64,
        cache_read: i64,
        cache_create_5m: i64,
        cache_create_1h: i64,
    ) -> Option<ModelCost> {
        let p = self.price(model_id)?;
        let c = |tokens: i64, rate: f64| (tokens as f64) * rate / 1_000_000.0;
        let one_hour_rate = p.cache_create_1h.unwrap_or(p.cache_create);
        Some(ModelCost {
            input: c(input, p.input),
            output: c(output, p.output),
            cache_read: c(cache_read, p.cache_read),
            cache_create: c(cache_create_5m, p.cache_create) + c(cache_create_1h, one_hour_rate),
        })
    }
}

fn user_overrides() -> HashMap<String, ModelPrice> {
    let Some(dir) = dirs::data_dir() else {
        return HashMap::new();
    };
    let path = dir.join("HPBar").join("pricing.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// `claude-haiku-4-5-20251001` → `claude-haiku-4-5` (drop trailing date stamp).
pub fn strip_date_suffix(id: &str) -> String {
    let mut parts: Vec<&str> = id.split('-').collect();
    while let Some(last) = parts.last() {
        if last.len() >= 6 && last.chars().all(|c| c.is_ascii_digit()) {
            parts.pop();
        } else {
            break;
        }
    }
    parts.join("-")
}
