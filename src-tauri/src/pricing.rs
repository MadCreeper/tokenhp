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
    pub long_context_above: Option<i64>,
    pub long_context: Option<Box<ModelPrice>>,
    /// DeepSeek's published UTC peak/off-peak schedule.
    pub off_peak_discount: Option<f64>,
}

impl ModelPrice {
    fn valid(&self) -> bool {
        [
            self.input,
            self.output,
            self.cache_read,
            self.cache_create,
            self.cache_create_1h.unwrap_or(self.cache_create),
        ]
        .iter()
        .all(|rate| rate.is_finite() && *rate >= 0.0)
            && self.long_context.as_ref().map_or(true, |p| p.valid())
            && self
                .off_peak_discount
                .map_or(true, |d| d.is_finite() && (0.0..=1.0).contains(&d))
    }
}

/// Per-component dollar cost for a model's usage.
#[derive(Clone, Copy, Debug, Default)]
pub struct ModelCost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_create: f64, // combined 5m + 1h
}

impl ModelCost {
    pub fn add(&mut self, other: Self) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_create += other.cache_create;
    }
    pub fn total(&self) -> f64 {
        self.input + self.output + self.cache_read + self.cache_create
    }
}

pub struct Pricing {
    table: HashMap<String, ModelPrice>,
}

impl Pricing {
    pub(crate) fn bundled() -> Self {
        Self {
            table: serde_json::from_str(BUNDLED).expect("valid bundled pricing"),
        }
    }

    /// Bundled table overlaid with user overrides. Malformed/missing files are
    /// tolerated (bundled wins nothing → empty; user missing → just bundled).
    pub fn loaded() -> Self {
        let mut table = Self::bundled().table;
        for (k, v) in user_overrides() {
            if v.valid() {
                table.insert(k.to_ascii_lowercase(), v);
            }
        }
        Pricing { table }
    }

    /// Exact ID (case-insensitive), then a dated snapshot of that exact model.
    /// Unknown variants stay unpriced: e.g. a new Opus must not inherit Opus 4's
    /// much higher price, and a new GPT Pro must not inherit the base rate.
    fn price(&self, model_id: &str) -> Option<&ModelPrice> {
        let id = model_id.to_ascii_lowercase();
        self.table
            .get(&id)
            .or_else(|| self.table.get(&strip_date_suffix(&id)))
    }

    /// Changes to bundled rates or local overrides invalidate fitting history.
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut rows: Vec<_> = self.table.iter().collect();
        rows.sort_by(|a, b| a.0.cmp(b.0));
        format!("{:x}", Sha256::digest(format!("{rows:?}").as_bytes()))[..16].to_string()
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
        Some(calculate(
            self.price(model_id)?,
            input,
            output,
            cache_read,
            cache_create_5m,
            cache_create_1h,
            1.0,
        ))
    }

    /// Price each request BEFORE aggregation: a day's tokens are not a single
    /// long-context prompt, and peak/off-peak rates use the event's UTC time.
    pub fn cost_request(&self, model: &str, tokens: [i64; 5], at: i64) -> Option<ModelCost> {
        use chrono::{Datelike, Timelike};
        let base = self.price(model)?;
        let prompt = tokens[0]
            .saturating_add(tokens[2])
            .saturating_add(tokens[3])
            .saturating_add(tokens[4]);
        let p = if base.long_context_above.is_some_and(|limit| prompt > limit) {
            base.long_context.as_deref().unwrap_or(base)
        } else {
            base
        };
        let off_peak = chrono::DateTime::from_timestamp(at, 0).is_some_and(|t| {
            t.weekday().number_from_monday() > 5
                || !((1..4).contains(&t.hour()) || (6..10).contains(&t.hour()))
        });
        let factor = if off_peak {
            base.off_peak_discount.unwrap_or(1.0)
        } else {
            1.0
        };
        Some(calculate(
            p, tokens[0], tokens[1], tokens[2], tokens[3], tokens[4], factor,
        ))
    }
}

fn calculate(
    p: &ModelPrice,
    input: i64,
    output: i64,
    read: i64,
    write: i64,
    write_1h: i64,
    factor: f64,
) -> ModelCost {
    let c = |tokens: i64, rate: f64| tokens.max(0) as f64 * rate * factor / 1_000_000.0;
    ModelCost {
        input: c(input, p.input),
        output: c(output, p.output),
        cache_read: c(read, p.cache_read),
        cache_create: c(write, p.cache_create)
            + c(write_1h, p.cache_create_1h.unwrap_or(p.cache_create)),
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

    fn bundled() -> Pricing {
        Pricing {
            table: serde_json::from_str(BUNDLED).unwrap(),
        }
    }

    #[test]
    fn long_context_is_priced_per_request_and_includes_cache() {
        let p = bundled();
        let short = p
            .cost_request("claude-haiku-5-5", [100_000, 10_000, 0, 0, 0], 0)
            .unwrap();
        assert!((short.total() - 0.015).abs() < 1e-9);
        let long = p
            .cost_request("claude-haiku-5-5", [1, 10_000, 100_000, 0, 0], 0)
            .unwrap();
        assert!((long.total() - 0.0300005).abs() < 1e-9);
        // Ten short requests retain the short rate even if their total is 1M.
        assert!((short.total() * 10.0 - 0.15).abs() < 1e-9);
    }

    #[test]
    fn deepseek_uses_utc_peak_schedule() {
        let p = bundled();
        let at = |s| chrono::DateTime::parse_from_rfc3339(s).unwrap().timestamp();
        let price = |s| {
            p.cost_request("deepseek-flash", [1_000_000, 0, 0, 0, 0], at(s))
                .unwrap()
                .total()
        };
        assert_eq!(price("2026-10-15T01:00:00Z"), 0.3);
        assert_eq!(price("2026-10-15T04:00:00Z"), 0.15);
        assert_eq!(price("2026-10-15T06:00:00Z"), 0.3);
        assert_eq!(price("2026-10-17T06:00:00Z"), 0.15);
    }

    #[test]
    fn snapshots_and_cache_ttl_rates_are_preserved() {
        let p = bundled();
        assert_eq!(strip_date_suffix("gpt-6.1-sol-2026-10-01"), "gpt-6.1-sol");
        assert_eq!(strip_date_suffix("claude-opus-5-5"), "claude-opus-5-5");
        let c = p
            .cost("claude-opus-5-5", 0, 0, 0, 1_000_000, 1_000_000)
            .unwrap();
        assert_eq!(c.total(), 13.0);
        assert_eq!(p.cost("gpt-6-sol", -100, 0, 0, 0, 0).unwrap().total(), 0.0);
    }

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
            "gpt-6-astra",
            "gpt-5.6-sol",
        ] {
            assert!(table.contains_key(id), "missing {id}");
        }
    }

    #[test]
    fn observed_team_models_are_priced_except_explicit_exclusions() {
        let sources: serde_json::Value =
            serde_json::from_str(include_str!("pricing-sources.json")).unwrap();
        let pricing = bundled();
        let models = sources["coverage"]["model_ids"].as_array().unwrap();
        assert_eq!(models.len(), 21);
        for model in models {
            let id = model.as_str().unwrap();
            if sources["coverage"]["intentionally_unpriced"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v.as_str() == Some(id))
            {
                assert!(
                    pricing.cost(id, 1, 1, 1, 0, 0).is_none(),
                    "{id} must stay unpriced"
                );
                continue;
            }
            let cost = pricing
                .cost_request(id, [1000, 1000, 1000, 0, 0], 0)
                .unwrap_or_else(|| panic!("Missing bundled rate for observed Team model {id}"));
            assert!(
                cost.input > 0.0 && cost.output > 0.0 && cost.cache_read > 0.0,
                "{id}"
            );
        }
    }

    #[test]
    fn team_aliases_and_legacy_model_match_the_documented_rates() {
        let p = bundled();
        for model in ["ATXC44Y9/gpt-6-astra", "8TZQGKP5/gpt-6-astra"] {
            for tokens in [[1000, 1000, 1000, 0, 0], [300_000, 1000, 1000, 0, 0]] {
                assert_eq!(
                    p.cost_request(model, tokens, 0).unwrap().total(),
                    p.cost_request("gpt-6-astra", tokens, 0).unwrap().total()
                );
            }
        }
        let max = p
            .cost("gpt-5.1-codex-max", 1_000_000, 1_000_000, 1_000_000, 0, 0)
            .unwrap();
        assert!((max.total() - 11.375).abs() < 1e-9);
        assert_eq!(p.price("gpt-6.1-sol").unwrap().cache_read, 0.1);
        assert_eq!(p.price("gpt-5.6-terra").unwrap().output, 12.0);
    }

    #[test]
    fn unknown_variants_are_not_priced_as_their_ancestors() {
        let pricing = Pricing {
            table: serde_json::from_str(BUNDLED).unwrap(),
        };
        assert!(pricing.cost("claude-opus-4-99", 1, 0, 0, 0, 0).is_none());
        assert!(pricing.cost("gpt-5.4-unknown", 1, 0, 0, 0, 0).is_none());
        assert!(pricing.cost("MiniMax-M3", 1, 0, 0, 0, 0).is_some());
        assert!(pricing
            .cost("claude-haiku-4-5-20251001", 1, 0, 0, 0, 0)
            .is_some());
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
            assert!(
                p.cache_read <= p.input,
                "{id}: cache read dearer than input?"
            );
        }
    }

    #[test]
    fn cost_is_per_million_tokens() {
        let table: HashMap<String, ModelPrice> = serde_json::from_str(BUNDLED).unwrap();
        let pricing = Pricing { table };
        // 1M input + 1M output on Opus 4.8 = $5 + $25.
        let cost = pricing
            .cost("claude-opus-4-8", 1_000_000, 1_000_000, 0, 0, 0)
            .unwrap();
        assert!((cost.total() - 30.0).abs() < 1e-9, "got {}", cost.total());
        // Date-suffixed IDs fall back to the base entry.
        assert!(pricing
            .cost("claude-haiku-4-5-20251001", 1_000_000, 0, 0, 0, 0)
            .is_some());
    }
}

/// `claude-haiku-4-5-20251001` → `claude-haiku-4-5` (drop trailing date stamp).
pub fn strip_date_suffix(id: &str) -> String {
    let mut parts: Vec<&str> = id.split('-').collect();
    if parts
        .last()
        .is_some_and(|s| s.len() == 8 && s.chars().all(|c| c.is_ascii_digit()))
    {
        parts.pop();
    } else if parts.len() >= 4 {
        let date = parts[parts.len() - 3..].join("-");
        if chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d").is_ok() {
            parts.truncate(parts.len() - 3);
        }
    }
    parts.join("-")
}
