//! Per-model token pricing. Port of the Swift `Pricing` / `ModelPrice`.
//!
//! Prices live in JSON, not code. Three sources, merged in precedence order:
//!   1. Bundled `pricing.json` (compiled in via `include_str!`) — offline defaults.
//!   2. Cached remote table at `<data_dir>/HPBar/pricing-cache.json`, refreshed
//!      periodically (with `ETag` / `If-None-Match`) from HPBar's GitHub repo.
//!   3. User overrides at `<data_dir>/HPBar/pricing.json` — always wins.
//!
//! All values are dollars per million tokens.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicI64, Ordering},
        OnceLock,
    },
    time::Duration,
};
use tauri::{Emitter, Manager};

const BUNDLED: &str = include_str!("pricing.json");
const BUNDLED_OVERRIDES: &str = include_str!("pricing-overrides.json");

/// Default curated pricing feed published by Stage A (`sync-pricing.yml`).
const DEFAULT_REMOTE_URL: &str =
    "https://raw.githubusercontent.com/MadCreeper/tokenhp/main/src-tauri/src/pricing.json";
const USER_AGENT: &str = "HPBar-Pricing";

/// Periodic background refresh cadence (24h).
pub const REFRESH_INTERVAL_SECS: i64 = 24 * 60 * 60;
/// Minimum spacing between on-demand refreshes triggered by unknown models (1h).
const UNKNOWN_MODEL_COOLDOWN_SECS: i64 = 60 * 60;
/// Safety ceiling on downloaded JSON payload size (2 MiB).
const MAX_REMOTE_BYTES: usize = 2 * 1024 * 1024;
/// Reject suspiciously truncated remote tables.
const MIN_REMOTE_MODELS: usize = 30;

static REFRESH_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
static LAST_UNKNOWN_TRIGGER: AtomicI64 = AtomicI64::new(0);

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct ModelPrice {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_create: f64,
    /// 1-hour-TTL cache writes (2× input for Anthropic). Falls back to
    /// `cache_create` when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_create_1h: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long_context_above: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long_context: Option<Box<ModelPrice>>,
    /// DeepSeek's published UTC peak/off-peak schedule.
    #[serde(skip_serializing_if = "Option::is_none")]
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

    /// Stronger runtime sanity guard applied to remotely fetched entries:
    /// catches 1000× unit typos, inverted input/output rates, or broken cache rates.
    pub(crate) fn plausible(&self) -> bool {
        if !self.valid() {
            return false;
        }
        let rates = [
            self.input,
            self.output,
            self.cache_read,
            self.cache_create,
            self.cache_create_1h.unwrap_or(0.0),
        ];
        if !rates.iter().all(|rate| (0.0..=500.0).contains(rate)) {
            return false;
        }
        if self.output < self.input || self.cache_read > self.input {
            return false;
        }
        if let Some(lc) = &self.long_context {
            if !self.long_context_above.is_some_and(|n| n > 0) || !lc.plausible() {
                return false;
            }
        }
        true
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

    /// Merge bundled → cached remote → user overrides. Remote entries must pass
    /// `plausible()` and cannot override explicit exclusions. User overrides
    /// only require `valid()` and always win.
    fn from_layers(
        bundled: HashMap<String, ModelPrice>,
        remote: HashMap<String, ModelPrice>,
        overrides: HashMap<String, ModelPrice>,
    ) -> Self {
        let excluded = excluded_models();
        let mut table: HashMap<String, ModelPrice> = bundled
            .into_iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v))
            .collect();
        for (k, v) in remote {
            let key = k.to_ascii_lowercase();
            if !excluded.contains(&key) && v.plausible() {
                table.insert(key, v);
            }
        }
        for (k, v) in overrides {
            if v.valid() {
                table.insert(k.to_ascii_lowercase(), v);
            }
        }
        Pricing { table }
    }

    /// Bundled table overlaid with cached remote rates and user overrides.
    /// Malformed/missing cache or override files are tolerated cleanly.
    pub fn loaded() -> Self {
        Self::from_layers(
            Self::bundled().table,
            read_remote_cache(),
            user_overrides(),
        )
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

    /// Changes to rates invalidate fitting history. Prefer
    /// [`Self::fingerprint_for_provider`] so adding unrelated models does not
    /// reset another provider's share-fitting series.
    pub fn fingerprint(&self) -> String {
        hash_rows(self.table.iter())
    }

    /// Provider-scoped rate fingerprint for `share.rs`: hashes only entries that
    /// belong to `provider` (e.g. `claude-*` for `"claude"`, OpenAI `gpt-*`/`o*`
    /// for `"codex"`), so automated pricing syncs for unrelated providers leave
    /// existing fitting histories intact.
    pub fn fingerprint_for_provider(&self, provider: &str) -> String {
        hash_rows(
            self.table
                .iter()
                .filter(|(id, _)| model_matches_provider(id, provider)),
        )
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

fn hash_rows<'a>(rows_iter: impl Iterator<Item = (&'a String, &'a ModelPrice)>) -> String {
    let mut rows: Vec<_> = rows_iter.collect();
    rows.sort_by(|a, b| a.0.cmp(b.0));
    format!("{:x}", Sha256::digest(format!("{rows:?}").as_bytes()))[..16].to_string()
}

fn model_matches_provider(model_id: &str, provider: &str) -> bool {
    let id = model_id.to_ascii_lowercase();
    let bare = id.rsplit_once('/').map_or(id.as_str(), |(_, rest)| rest);
    match provider {
        "claude" => bare.starts_with("claude-"),
        "codex" => {
            bare.starts_with("gpt-")
                || bare.starts_with("chatgpt-")
                || bare.starts_with("codex-")
                || bare == "o1"
                || bare.starts_with("o1-")
                || bare == "o3"
                || bare.starts_with("o3-")
                || bare == "o4"
                || bare.starts_with("o4-")
        }
        _ => true,
    }
}

fn excluded_models() -> &'static HashSet<String> {
    static EXCLUDED: OnceLock<HashSet<String>> = OnceLock::new();
    EXCLUDED.get_or_init(|| {
        #[derive(Deserialize)]
        struct OverridesMeta {
            #[serde(default)]
            exclude: Vec<String>,
        }
        serde_json::from_str::<OverridesMeta>(BUNDLED_OVERRIDES)
            .map(|m| m.exclude.into_iter().map(|s| s.to_ascii_lowercase()).collect())
            .unwrap_or_default()
    })
}

/// True when `model_id` is explicitly excluded from automatic pricing (e.g.
/// `codex-auto-review`), so encountering it does not trigger remote refreshes.
pub fn is_intentionally_unpriced(model_id: &str) -> bool {
    let id = model_id.to_ascii_lowercase();
    let excluded = excluded_models();
    excluded.contains(&id) || excluded.contains(&strip_date_suffix(&id))
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

fn app_data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("HPBar"))
}

fn cache_paths_in(dir: &Path) -> (PathBuf, PathBuf) {
    (
        dir.join("pricing-cache.json"),
        dir.join("pricing-cache-meta.json"),
    )
}

fn read_remote_cache_from(dir: &Path) -> HashMap<String, ModelPrice> {
    let (cache_path, _) = cache_paths_in(dir);
    std::fs::read_to_string(cache_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn read_remote_cache() -> HashMap<String, ModelPrice> {
    app_data_dir()
        .map(|d| read_remote_cache_from(&d))
        .unwrap_or_default()
}

fn user_overrides() -> HashMap<String, ModelPrice> {
    let Some(dir) = app_data_dir() else {
        return HashMap::new();
    };
    let path = dir.join("pricing.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

// --- Stage B: Conditional remote cache refresh -------------------------------

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
struct CacheMeta {
    etag: Option<String>,
    fetched_at: i64,
    sha256: String,
    model_count: usize,
}

#[derive(Serialize, Clone, Debug)]
pub struct PricingRefreshResult {
    pub updated: bool,
    pub not_modified: bool,
    pub model_count: usize,
    pub fetched_at: i64,
}

fn read_cache_meta(dir: &Path) -> Option<CacheMeta> {
    let (_, meta_path) = cache_paths_in(dir);
    std::fs::read_to_string(meta_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Couldn't create pricing cache dir: {e}"))?;
    }
    let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    std::fs::write(&tmp, bytes).map_err(|e| format!("Couldn't write pricing cache: {e}"))?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("Couldn't persist pricing cache: {e}"));
    }
    Ok(())
}

fn remote_url() -> String {
    std::env::var("HPBAR_PRICING_URL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_REMOTE_URL.to_string())
}

fn validate_remote_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw).map_err(|_| "Invalid pricing feed URL.".to_string())?;
    if url.scheme() == "https" {
        return Ok(url);
    }
    #[cfg(test)]
    if url.scheme() == "http"
        && matches!(url.host_str(), Some("127.0.0.1" | "localhost"))
    {
        return Ok(url);
    }
    Err("Pricing feed URL must use HTTPS.".into())
}

fn parse_and_validate_remote(bytes: &[u8]) -> Result<HashMap<String, ModelPrice>, String> {
    if bytes.is_empty() || bytes.len() > MAX_REMOTE_BYTES {
        return Err("Remote pricing payload size out of bounds.".into());
    }
    let raw: HashMap<String, ModelPrice> =
        serde_json::from_slice(bytes).map_err(|e| format!("Invalid pricing JSON: {e}"))?;
    let excluded = excluded_models();
    let mut cleaned = HashMap::with_capacity(raw.len());
    for (k, v) in raw {
        let key = k.to_ascii_lowercase();
        if excluded.contains(&key) {
            continue;
        }
        if !v.plausible() {
            return Err(format!("Remote pricing entry '{key}' failed plausibility checks."));
        }
        cleaned.insert(key, v);
    }
    if cleaned.len() < MIN_REMOTE_MODELS {
        return Err(format!(
            "Remote pricing table looks truncated ({} models).",
            cleaned.len()
        ));
    }
    Ok(cleaned)
}

async fn refresh_cache_into(
    dir: &Path,
    url_str: &str,
    min_age_secs: Option<i64>,
    client: &reqwest::Client,
) -> Result<PricingRefreshResult, String> {
    let _guard = REFRESH_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;

    let now = chrono::Utc::now().timestamp();
    let prev_meta = read_cache_meta(dir);

    if let (Some(min_age), Some(meta)) = (min_age_secs, &prev_meta) {
        if now.saturating_sub(meta.fetched_at) < min_age {
            return Ok(PricingRefreshResult {
                updated: false,
                not_modified: true,
                model_count: meta.model_count,
                fetched_at: meta.fetched_at,
            });
        }
    }

    let url = validate_remote_url(url_str)?;

    let mut req = client.get(url).header("User-Agent", USER_AGENT);
    if let Some(etag) = prev_meta.as_ref().and_then(|m| m.etag.as_deref()) {
        req = req.header("If-None-Match", etag);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("Pricing refresh network error: {e}"))?;

    let (cache_path, meta_path) = cache_paths_in(dir);

    if resp.status().as_u16() == 304 {
        let mut meta = prev_meta.unwrap_or_default();
        meta.fetched_at = now;
        let meta_bytes =
            serde_json::to_vec_pretty(&meta).map_err(|e| format!("Meta encode error: {e}"))?;
        atomic_write(&meta_path, &meta_bytes)?;
        return Ok(PricingRefreshResult {
            updated: false,
            not_modified: true,
            model_count: meta.model_count,
            fetched_at: now,
        });
    }

    if !resp.status().is_success() {
        return Err(format!(
            "Pricing feed returned HTTP {}.",
            resp.status().as_u16()
        ));
    }

    let etag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Pricing download failed: {e}"))?;

    let validated = parse_and_validate_remote(&bytes)?;
    let normalized_bytes =
        serde_json::to_vec(&validated).map_err(|e| format!("Pricing serialize error: {e}"))?;
    let sha = format!("{:x}", Sha256::digest(&normalized_bytes));
    let updated = prev_meta.as_ref().map(|m| m.sha256.as_str()) != Some(sha.as_str());

    if updated || !cache_path.is_file() {
        atomic_write(&cache_path, &normalized_bytes)?;
    }

    let meta = CacheMeta {
        etag,
        fetched_at: now,
        sha256: sha,
        model_count: validated.len(),
    };
    let meta_bytes =
        serde_json::to_vec_pretty(&meta).map_err(|e| format!("Meta encode error: {e}"))?;
    atomic_write(&meta_path, &meta_bytes)?;

    Ok(PricingRefreshResult {
        updated,
        not_modified: false,
        model_count: meta.model_count,
        fetched_at: now,
    })
}

/// Refresh cached pricing from GitHub if at least `min_age_secs` have elapsed
/// (pass `None` to force an unconditional ETag check).
pub async fn refresh_cache(min_age_secs: Option<i64>) -> Result<PricingRefreshResult, String> {
    let dir = app_data_dir().ok_or_else(|| "Couldn't locate app data directory.".to_string())?;
    let client = crate::network::client(
        crate::network::login_shell_proxy().as_deref(),
        Duration::from_secs(15),
    )
    .map_err(|e| format!("Couldn't initialize HTTP client: {e}"))?;
    refresh_cache_into(&dir, &remote_url(), min_age_secs, &client).await
}

/// Manual / UI-triggered pricing refresh command.
#[tauri::command]
pub async fn refresh_pricing(app: tauri::AppHandle) -> Result<PricingRefreshResult, String> {
    let result = refresh_cache(None).await?;
    if result.updated {
        if let Some(win) = app.get_webview_window("popover") {
            let _ = win.emit("refresh", ());
        }
    }
    Ok(result)
}

/// Background refresher spawned at app startup: waits briefly after launch,
/// then checks every 6 hours whether the cached pricing table is older than 24h.
pub fn spawn_refresher(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(15)).await;
        loop {
            if let Ok(res) = refresh_cache(Some(REFRESH_INTERVAL_SECS)).await {
                if res.updated {
                    if let Some(win) = app.get_webview_window("popover") {
                        let _ = win.emit("refresh", ());
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(6 * 60 * 60)).await;
        }
    });
}

/// On-demand refresh when local log scanning spots an unpriced model ID that is
/// not intentionally excluded. Rate-limited to once per hour so an unknown model
/// cannot spam GitHub.
pub fn trigger_refresh_for_unknown_models(app: tauri::AppHandle, unpriced_ids: &[String]) {
    if !unpriced_ids.iter().any(|id| !is_intentionally_unpriced(id)) {
        return;
    }
    let now = chrono::Utc::now().timestamp();
    let prev = LAST_UNKNOWN_TRIGGER.load(Ordering::Relaxed);
    if now.saturating_sub(prev) < UNKNOWN_MODEL_COOLDOWN_SECS {
        return;
    }
    if LAST_UNKNOWN_TRIGGER
        .compare_exchange(prev, now, Ordering::SeqCst, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    tauri::async_runtime::spawn(async move {
        if let Ok(res) = refresh_cache(Some(UNKNOWN_MODEL_COOLDOWN_SECS)).await {
            if res.updated {
                if let Some(win) = app.get_webview_window("popover") {
                    let _ = win.emit("refresh", ());
                }
            }
        }
    });
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
            assert!(p.plausible(), "{id} failed ModelPrice::plausible()");
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

    fn sample_price(input: f64, output: f64) -> ModelPrice {
        ModelPrice {
            input,
            output,
            cache_read: input * 0.1,
            cache_create: 0.0,
            cache_create_1h: None,
            long_context_above: None,
            long_context: None,
            off_peak_discount: None,
        }
    }

    #[test]
    fn three_tier_merge_respects_precedence_plausibility_and_exclusions() {
        let bundled = HashMap::from([("model-a".into(), sample_price(1.0, 5.0))]);
        let remote = HashMap::from([
            ("model-a".into(), sample_price(2.0, 10.0)),
            ("model-remote".into(), sample_price(3.0, 15.0)),
            // Implausible remote entry (output < input) must be rejected.
            ("model-bad".into(), sample_price(10.0, 2.0)),
            // Excluded model must never be introduced by remote cache.
            ("codex-auto-review".into(), sample_price(1.0, 5.0)),
        ]);
        let overrides = HashMap::from([("model-a".into(), sample_price(4.0, 20.0))]);

        let merged = Pricing::from_layers(bundled, remote, overrides);
        assert_eq!(merged.price("model-a").unwrap().input, 4.0);
        assert_eq!(merged.price("model-remote").unwrap().input, 3.0);
        assert!(merged.price("model-bad").is_none());
        assert!(merged.price("codex-auto-review").is_none());
    }

    #[test]
    fn provider_scoped_fingerprint_ignores_unrelated_providers() {
        let base = Pricing::bundled();
        let claude_fp = base.fingerprint_for_provider("claude");
        let codex_fp = base.fingerprint_for_provider("codex");

        let mut with_gemini = base.table.clone();
        with_gemini.insert("gemini-4-neon".into(), sample_price(3.0, 15.0));
        with_gemini.insert("glm-5.4".into(), sample_price(1.4, 4.4));
        let p_gemini = Pricing { table: with_gemini };

        assert_eq!(p_gemini.fingerprint_for_provider("claude"), claude_fp);
        assert_eq!(p_gemini.fingerprint_for_provider("codex"), codex_fp);

        let mut with_claude = base.table.clone();
        with_claude.insert("claude-opus-9".into(), sample_price(4.0, 20.0));
        let p_claude = Pricing { table: with_claude };
        assert_ne!(p_claude.fingerprint_for_provider("claude"), claude_fp);
        assert_eq!(p_claude.fingerprint_for_provider("codex"), codex_fp);

        let mut with_codex = base.table.clone();
        with_codex.insert("gpt-9-astra".into(), sample_price(8.0, 40.0));
        let p_codex = Pricing { table: with_codex };
        assert_eq!(p_codex.fingerprint_for_provider("claude"), claude_fp);
        assert_ne!(p_codex.fingerprint_for_provider("codex"), codex_fp);
    }

    #[test]
    fn remote_validation_rejects_truncated_and_implausible_payloads() {
        assert!(parse_and_validate_remote(b"{}").is_err());
        let mut bad: HashMap<String, ModelPrice> = serde_json::from_str(BUNDLED).unwrap();
        bad.insert("broken-model".into(), sample_price(50.0, 1.0));
        let bytes = serde_json::to_vec(&bad).unwrap();
        assert!(parse_and_validate_remote(&bytes).is_err());

        let mut with_excluded: HashMap<String, ModelPrice> =
            serde_json::from_str(BUNDLED).unwrap();
        with_excluded.insert("codex-auto-review".into(), sample_price(1.0, 5.0));
        let cleaned =
            parse_and_validate_remote(&serde_json::to_vec(&with_excluded).unwrap()).unwrap();
        assert!(!cleaned.contains_key("codex-auto-review"));
        assert!(is_intentionally_unpriced("codex-auto-review"));
        assert!(!is_intentionally_unpriced("claude-opus-9"));
    }

    #[tokio::test]
    async fn refresh_cache_handles_etag_304_and_atomic_persistence() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = BUNDLED.as_bytes().to_vec();

        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
                if req.contains("if-none-match: \"v1\"") {
                    let resp = "HTTP/1.1 304 Not Modified\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    sock.write_all(resp.as_bytes()).await.unwrap();
                } else {
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nETag: \"v1\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    sock.write_all(header.as_bytes()).await.unwrap();
                    sock.write_all(&body).await.unwrap();
                }
            }
        });

        let tmp = std::env::temp_dir().join(format!("hpbar-pricing-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let url = format!("http://{addr}/pricing.json");
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();

        // First fetch: 200 OK → persists cache + meta
        let r1 = refresh_cache_into(&tmp, &url, None, &client).await.unwrap();
        assert!(r1.updated);
        assert!(!r1.not_modified);
        assert!(r1.model_count > 30);
        assert!(!read_remote_cache_from(&tmp).is_empty());

        // Min-age gate skips network completely
        let r_skip = refresh_cache_into(&tmp, &url, Some(3600), &client)
            .await
            .unwrap();
        assert!(!r_skip.updated);
        assert!(r_skip.not_modified);

        // Forced check sends If-None-Match → 304 Not Modified
        let r2 = refresh_cache_into(&tmp, &url, None, &client).await.unwrap();
        assert!(!r2.updated);
        assert!(r2.not_modified);

        server.await.unwrap();
        let _ = std::fs::remove_dir_all(&tmp);
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
