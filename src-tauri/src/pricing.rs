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

    /// Exact id, then the id without its date stamp, then the model's family
    /// (`claude-fable-5-1` → `claude-fable-5`): a point release the table
    /// doesn't know yet prices at its predecessor's rate instead of $0 — which
    /// silently zeroed this machine's local cost the day a new model landed,
    /// and with it the device-share split. Explicit entries still win.
    fn price(&self, model_id: &str) -> Option<&ModelPrice> {
        if let Some(p) = self.table.get(model_id) {
            return Some(p);
        }
        let base = strip_date_suffix(model_id);
        if let Some(p) = self.table.get(&base) {
            return Some(p);
        }
        let mut parts: Vec<&str> = base.split('-').collect();
        // Stop short of the bare family name (`claude-opus`): nothing that
        // generic is a price we'd want to assume.
        while parts.len() > 3 {
            parts.pop();
            if let Some(p) = self.table.get(&parts.join("-")) {
                return Some(p);
            }
        }
        None
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `Pricing::loaded()` tolerates a malformed table by falling back to empty,
    /// which would silently zero every cost. Fail loudly here instead.
    #[test]
    fn bundled_table_parses() {
        let table: HashMap<String, ModelPrice> =
            serde_json::from_str(BUNDLED).expect("pricing.json is not valid JSON");
        assert!(table.len() > 30, "bundled table looks truncated");
        for id in [
            "claude-opus-4-8",
            "claude-opus-5",
            "claude-fable-5",
            "claude-fable-5-1",
            "kimi-k3",
            "glm-5.2",
        ] {
            assert!(table.contains_key(id), "missing {id}");
        }
    }

    /// An unknown point release prices at its family's rate rather than
    /// vanishing from every cost total; the bare family name never matches.
    #[test]
    fn unknown_point_release_falls_back_to_its_family() {
        let table: HashMap<String, ModelPrice> = serde_json::from_str(BUNDLED).unwrap();
        let pricing = Pricing { table };
        let base = pricing
            .cost("claude-fable-5", 1_000_000, 0, 0, 0, 0)
            .unwrap();
        let next = pricing
            .cost("claude-fable-5-9", 1_000_000, 0, 0, 0, 0)
            .expect("falls back to claude-fable-5");
        assert!((base.total() - next.total()).abs() < 1e-9);
        let dated = pricing.cost("claude-fable-5-9-20270101", 1, 0, 0, 0, 0);
        assert!(dated.is_some());
        assert!(pricing.cost("claude-nova-7", 1, 0, 0, 0, 0).is_none());
        assert!(pricing.cost("gpt-9.1-sol", 1, 0, 0, 0, 0).is_none());
    }

    /// Every rate is a sane, positive-or-zero dollars-per-million figure — a
    /// stray factor of 1000 (or a per-token rate pasted in) shows up here.
    #[test]
    fn rates_are_plausible() {
        let table: HashMap<String, ModelPrice> = serde_json::from_str(BUNDLED).unwrap();
        for (id, p) in &table {
            for (label, rate) in [
                ("input", p.input),
                ("output", p.output),
                ("cache_read", p.cache_read),
                ("cache_create", p.cache_create),
                ("cache_create_1h", p.cache_create_1h.unwrap_or(0.0)),
            ] {
                assert!(
                    (0.0..=500.0).contains(&rate),
                    "{id}.{label} = {rate} is out of range"
                );
            }
            assert!(p.output >= p.input, "{id}: output cheaper than input?");
            assert!(p.cache_read <= p.input, "{id}: cache read dearer than input?");
        }
    }

    #[test]
    fn cost_is_per_million_tokens() {
        let table: HashMap<String, ModelPrice> = serde_json::from_str(BUNDLED).unwrap();
        let pricing = Pricing { table };
        // 1M input + 1M output on Opus 4.8 = $5 + $25.
        let cost = pricing.cost("claude-opus-4-8", 1_000_000, 1_000_000, 0, 0, 0).unwrap();
        assert!((cost.total() - 30.0).abs() < 1e-9, "got {}", cost.total());
        // Date-suffixed IDs fall back to the base entry.
        assert!(pricing.cost("claude-haiku-4-5-20251001", 1_000_000, 0, 0, 0, 0).is_some());
    }
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
