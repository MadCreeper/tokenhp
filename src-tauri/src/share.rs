//! "What % of the plan is this machine using?" — fits the account-wide
//! utilization the provider reports against this machine's local cost, to split
//! each window into **this machine** vs **other devices**.
//!
//! The provider's API/local data gives account-wide `U` (0..1) per window; the
//! local logs only see this machine. There is no device id, so other devices are
//! *inferred*: for a window with budget `Q` (the dollar-cost that equals 100%),
//!
//! ```text
//! U ≈ (cost_this_machine + cost_other_devices) / Q
//! this_machine = cost_this_machine / Q ,   others = U − this_machine
//! ```
//!
//! API-equivalent cost is a proxy, not the provider's published metering formula.
//! Fit contiguous intervals so delayed quota updates are paired with pending
//! local spend. Automatic robust regression uses changes in activity to separate
//! local usage from background account usage. Unidentifiable fits stay hidden. Account,
//! pricing, reset and duration boundaries prevent unrelated histories mixing.

use crate::usage::UsageReport;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::{AppHandle, Manager};

/// One observation for a (provider, window): account-wide `u` (0..1) and this
/// machine's cumulative cost since the window's last reset, at unix `ts`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ShareSample {
    pub ts: i64,
    #[serde(default)]
    pub reset_ts: Option<i64>,
    pub u: f64,
    pub local_cost: f64,
    /// Legacy field retained for reading old histories; never trusted by the fit.
    #[serde(default)]
    pub calibrated: bool,
}

/// Preserve interval boundaries; thinning cumulative samples changes the fit.
const SHARE_RETAIN_SECS: i64 = 64 * 86_400;
const MIN_DU: f64 = 0.05;
const MAX_SAMPLE_AGE: i64 = 15 * 60;
const MAX_SAMPLE_GAP: i64 = 30 * 60;
const SUPPORT_TOL: f64 = 0.25;

/// Claude's ambient poll and a foreground Codex refresh can update the shared
/// history file at the same time. Serialize the whole read/modify/write cycle;
/// otherwise the later rename can silently discard the other provider's new
/// sample and make the displayed split appear to refresh intermittently.
static SHARE_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();

// ---- window descriptors -----------------------------------------------------

/// Seconds spanned by a window title: "5-Hour"/"N-Hour" → N·3600, "Weekly…"
/// (incl. per-model caps like "Weekly (Fable)") → 7d, "Monthly" → 30d; default
/// 5h. Used to align local cost to the server window and as the recency
/// half-life (one cycle).
pub fn window_secs_for_title(title: &str) -> i64 {
    if let Some(h) = title.strip_suffix("-Hour") {
        if let Ok(n) = h.trim().parse::<i64>() {
            return (n.max(1)) * 3600;
        }
    }
    if title.starts_with("Weekly") {
        return 7 * 86_400;
    }
    if title.starts_with("Monthly") {
        return 30 * 86_400;
    }
    5 * 3600
}

// ---- the estimator (pure, unit-tested) --------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct ShareResult {
    pub this_machine: f64, // 0..1 of the window
    pub others: f64,       // 0..1
    pub q: f64,            // estimated window budget in local $
    pub confidence: f64,   // 0..1
}

/// Estimate this machine's share of a window from its recorded series + the live
/// utilization `live_u`. `cycle_secs` = window length (recency half-life).
/// Returns `None` when there's no usable data at all.
pub fn estimate(
    samples: &[ShareSample],
    now: i64,
    live_u: f64,
    cycle_secs: i64,
) -> Option<ShareResult> {
    let current = samples.last()?;
    if cycle_secs <= 0
        || !live_u.is_finite()
        || !(0.0..1.0).contains(&live_u)
        || now < current.ts
        || now - current.ts > MAX_SAMPLE_AGE
        || !current.local_cost.is_finite()
        || current.local_cost < 0.0
        || (live_u - current.u).abs() > 0.015
        || current.reset_ts.is_some_and(|reset| reset <= now)
    {
        return None;
    }

    let mut ratios = Vec::new();
    let (mut acc_c, mut acc_u) = (0.0, 0.0);
    for pair in samples.windows(2) {
        let (p, s) = (&pair[0], &pair[1]);
        if !valid_sample(p)
            || !valid_sample(s)
            || s.ts <= p.ts
            || s.ts > now
            || s.ts - p.ts > MAX_SAMPLE_GAP
            || s.reset_ts != p.reset_ts
            || s.local_cost < p.local_cost
            || s.u < p.u
            || p.u >= 0.99
            || s.u >= 0.99
        {
            acc_c = 0.0;
            acc_u = 0.0;
            continue;
        }
        let dc = s.local_cost - p.local_cost;
        // Idle-only stretches provide no slope. Once spend is pending, retain
        // ALL quota changes, including delayed updates on idle polls.
        if acc_c == 0.0 && dc <= 1e-9 {
            continue;
        }
        acc_c += dc;
        acc_u += s.u - p.u;
        if acc_u + 1e-9 < MIN_DU {
            continue;
        }
        let ratio = acc_c / acc_u;
        let recency = 0.5_f64.powf((now - s.ts) as f64 / cycle_secs as f64);
        let evidence = (ratio, acc_u * recency);
        ratios.push(evidence);
        acc_c = 0.0;
        acc_u = 0.0;
    }

    // The ratio envelope is a useful provisional value, but cannot distinguish
    // two devices working in fixed proportion. Only an identifiable automatic
    // fit may graduate into the visible split.
    let automatic = automatic_fit(samples, now, cycle_secs);
    let q = automatic
        .map(|f| f.0)
        .or_else(|| weighted_percentile(&ratios, 0.8))?;
    if !q.is_finite() || q <= 0.0 {
        return None;
    }
    let mut confidence = automatic.map_or(0.2, |f| f.1);
    // New evidence contradicting a previously learned budget hides the split
    // promptly instead of continuing to attribute impossible local usage.
    if ratios.iter().rev().take(3).count() == 3
        && ratios.iter().rev().take(3).all(|(r, _)| *r > q * 1.5)
    {
        confidence = confidence.min(0.15);
    }
    let raw_this = current.local_cost / q;
    // An impossible split indicates lag, a changed plan/model mix or bad local
    // coverage. Do not conceal it by confidently clamping to "all this machine".
    if raw_this > live_u + 0.02 {
        confidence = confidence.min(0.15);
    }
    let this_machine = raw_this.clamp(0.0, live_u);
    Some(ShareResult {
        this_machine,
        others: live_u - this_machine,
        q,
        confidence,
    })
}

/// Ten-minute blocks absorb polling/rounding jitter. Fit quota/minute =
/// local $/minute * slope + background quota/minute with Theil–Sen regression.
/// Idle blocks participate as evidence, never as repeated cumulative ratios.
/// Recent halves must independently agree: old plans or one lucky burst cannot
/// make a confident split. This is a heuristic, not provider billing metering.
fn automatic_fit(samples: &[ShareSample], now: i64, cycle: i64) -> Option<(f64, f64)> {
    let mut blocks = Vec::new();
    let (mut cost, mut quota, mut elapsed) = (0.0, 0.0, 0i64);
    for pair in samples.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if !valid_sample(&a)
            || !valid_sample(&b)
            || b.ts <= a.ts
            || b.ts > now
            || now - a.ts > cycle.min(2 * 86400)
            || b.ts - a.ts > MAX_SAMPLE_GAP
            || a.reset_ts != b.reset_ts
            || b.local_cost < a.local_cost
            || b.u < a.u
            || b.u >= 0.99
        {
            cost = 0.0;
            quota = 0.0;
            elapsed = 0;
            continue;
        }
        cost += b.local_cost - a.local_cost;
        quota += b.u - a.u;
        elapsed += b.ts - a.ts;
        if elapsed >= 600 {
            let minutes = elapsed as f64 / 60.0;
            blocks.push((cost / minutes, quota / minutes, quota));
            cost = 0.0;
            quota = 0.0;
            elapsed = 0;
        }
    }
    let blocks = &blocks[blocks.len().saturating_sub(96)..];
    if blocks.len() < 8 || blocks.iter().filter(|b| b.0 > 1e-9).count() < 4 {
        return None;
    }
    let support: f64 = blocks.iter().map(|b| b.2).sum();
    if support < 0.10 {
        return None;
    }
    let (slope, error) = robust_slope(blocks)?;
    let mid = blocks.len() / 2;
    let early = robust_slope(&blocks[..mid])?.0;
    let late = robust_slope(&blocks[mid..])?.0;
    if (early / late - 1.0).abs() > SUPPORT_TOL || (late / slope - 1.0).abs() > SUPPORT_TOL {
        return None;
    }
    Some((
        1.0 / slope,
        (support / 0.25).min(1.0) * (1.0 - error).powi(2),
    ))
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let n = values.len();
    Some((values[(n - 1) / 2] + values[n / 2]) / 2.0)
}

fn robust_slope(blocks: &[(f64, f64, f64)]) -> Option<(f64, f64)> {
    let max_x = blocks.iter().map(|b| b.0).fold(0.0_f64, f64::max);
    let min_x = blocks.iter().map(|b| b.0).fold(f64::INFINITY, f64::min);
    // Constant local activity is not identifiable, regardless of sample count.
    if max_x <= 1e-9 || max_x - min_x < max_x * 0.4 {
        return None;
    }
    let mut slopes = Vec::new();
    for (i, a) in blocks.iter().enumerate() {
        for b in &blocks[i + 1..] {
            if (a.0 - b.0).abs() >= max_x * 0.25 {
                slopes.push((a.1 - b.1) / (a.0 - b.0));
            }
        }
    }
    let slope = median(slopes)?;
    if !slope.is_finite() || slope <= 0.0 {
        return None;
    }
    let intercept = median(blocks.iter().map(|b| b.1 - slope * b.0).collect())?;
    let total_y: f64 = blocks.iter().map(|b| b.1).sum();
    if total_y <= 0.0 || intercept < -0.0005 {
        return None;
    }
    let error = blocks
        .iter()
        .map(|b| (b.1 - slope * b.0 - intercept.max(0.0)).abs())
        .sum::<f64>()
        / total_y;
    if error > 0.15 {
        return None;
    }
    Some((slope, error))
}

fn valid_sample(s: &ShareSample) -> bool {
    s.u.is_finite() && (0.0..=1.0).contains(&s.u) && s.local_cost.is_finite() && s.local_cost >= 0.0
}

/// Weighted step percentile of `(value, weight)` pairs (value ascending).
fn weighted_percentile(pairs: &[(f64, f64)], p: f64) -> Option<f64> {
    let mut v: Vec<(f64, f64)> = pairs
        .iter()
        .copied()
        .filter(|(v, w)| v.is_finite() && w.is_finite() && *v > 0.0 && *w > 0.0)
        .collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let total: f64 = v.iter().map(|(_, w)| w).sum();
    let target = p * total;
    let mut cum = 0.0;
    for (val, w) in &v {
        cum += w;
        if cum >= target {
            return Some(*val);
        }
    }
    Some(v.last().unwrap().0)
}

// ---- file-backed series + record/annotate (mirrors ambient.rs patterns) -----

fn now_unix() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn parse_rfc3339(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp())
}

fn key(provider: &str, title: &str, account: &str, cycle: i64, pricing: &str) -> String {
    let parser = if provider == "codex" {
        format!("/parser-{}", crate::codexstats::PARSER_REVISION)
    } else {
        String::new()
    };
    format!("v4/{provider}{parser}/{account}/{cycle}/{pricing}/{title}")
}

fn current_account(provider: &str) -> Option<String> {
    match provider {
        "codex" => crate::account::codex_identity(),
        "claude" => crate::account::claude_identity(),
        _ => None,
    }
    .map(|i| i.account_key)
}

fn cycle_secs(w: &crate::usage::UsageWindow) -> i64 {
    w.window_minutes
        .filter(|m| *m > 0)
        .and_then(|m| m.checked_mul(60))
        .unwrap_or_else(|| window_secs_for_title(&w.title))
}

fn history_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    let dir = app.path().app_config_dir().ok()?;
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("share_history.json"))
}

fn load_history(app: &AppHandle) -> HashMap<String, Vec<ShareSample>> {
    history_path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_history(app: &AppHandle, h: &HashMap<String, Vec<ShareSample>>) {
    let Some(p) = history_path(app) else { return };
    let Ok(json) = serde_json::to_string(h) else {
        return;
    };
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

fn prune_and_thin(v: &mut Vec<ShareSample>, now: i64) {
    v.retain(|s| s.ts <= now && now - s.ts <= SHARE_RETAIN_SECS && valid_sample(s));
}

/// Align an observation with a still-active provider reset boundary. An expired
/// or implausible reset is ambiguous; wait for a fresh report instead of pairing
/// previous-cycle utilization with new-cycle cost.
pub fn window_span(reset_ts: i64, window_secs: i64, at: i64) -> Option<(i64, i64)> {
    if window_secs <= 0 || reset_ts <= at || reset_ts - at > window_secs {
        return None;
    }
    Some((reset_ts - window_secs, at))
}

/// This machine's cost over the span the provider's window covers as of `at`
/// (see [`window_span`]). `None` when the window can't be aligned. Scans logs →
/// call off the UI thread. Excludes API-key tools (they don't consume the
/// subscription).
fn local_cost_in_window(
    provider: &str,
    account: &str,
    reset_ts: i64,
    cycle: i64,
    at: i64,
) -> Option<f64> {
    let (start, end) = window_span(reset_ts, cycle, at)?;
    // Logs use integer seconds: include the quota snapshot's own final turn.
    let models = match provider {
        "codex" => {
            crate::codexstats::collect_local_range_for_account(start, end + 1, Some(account))?
        }
        _ => crate::localstats::collect_range_for_account(start, end + 1, Some(account))?,
    };
    let mut cost = 0.0;
    for model in models {
        if model.unattributed > 0 {
            return None;
        }
        cost += model.cost?.total;
    }
    Some(cost)
}

/// Append a sample per resettable window, observed now. Scans logs (heavy) —
/// caller should run it on a blocking thread.
pub fn record(app: &AppHandle, provider: &str, report: &UsageReport) {
    record_at(app, provider, report, now_unix());
}

/// Append a sample per resettable window for a report that describes the
/// account as of `at` — the time the figures were *observed*, which for a
/// Codex rollout snapshot is the last turn, not now. The local cost is aligned
/// to that instant, and a window that already has a sample at or after `at` is
/// left alone: the same snapshot re-read on every poll is one observation, not
/// a fresh one each time (re-stamping it "now" used to manufacture hundreds of
/// identical samples a day and, once the reset passed, misaligned their cost).
pub fn record_at(app: &AppHandle, provider: &str, report: &UsageReport, at: i64) {
    let _guard = SHARE_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut hist = load_history(app);
    let now = now_unix();
    if at > now || now - at > MAX_SAMPLE_AGE {
        return;
    }
    let Some(account) = current_account(provider) else {
        return;
    };
    let pricing = crate::pricing::Pricing::loaded().fingerprint();
    let calibrated = false; // legacy user override is intentionally ignored
    for w in &report.windows {
        if w.trailing.as_deref() == Some("Off") || w.resets_at.is_none() {
            continue;
        }
        // Per-model windows can't be fitted: the local-cost series is
        // account-wide, so their implied-budget ratio would be inflated.
        if crate::usage::is_model_scoped_title(&w.title) {
            continue;
        }
        let cycle = cycle_secs(w);
        let Some(reset_ts) = w.resets_at.as_deref().and_then(parse_rfc3339) else {
            continue;
        };
        let k = key(provider, &w.title, &account, cycle, &pricing);
        if hist
            .get(&k)
            .and_then(|v| v.last())
            .is_some_and(|last| at <= last.ts)
        {
            continue; // already observed this instant (or something newer)
        }
        let Some(cost) = local_cost_in_window(provider, &account, reset_ts, cycle, at) else {
            hist.remove(&k); // never annotate a previous, now incomplete sample
            continue;
        };
        hist.entry(k).or_default().push(ShareSample {
            ts: at,
            reset_ts: Some(reset_ts),
            u: w.utilization,
            local_cost: cost,
            calibrated,
        });
    }
    // Prune every series, not just the ones touched: a window the provider
    // stopped reporting (a retired "Monthly" bar, a per-model cap) otherwise
    // sits in the file forever.
    hist.retain(|_, v| {
        prune_and_thin(v, now);
        !v.is_empty()
    });
    save_history(app, &hist);
}

/// Fill each window's share fields from the recorded series. Light (reads the
/// small history file + pure math; no log scan) — safe on the command path.
pub fn annotate(app: &AppHandle, provider: &str, report: &mut UsageReport) {
    let _guard = SHARE_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let hist = load_history(app);
    let now = now_unix();
    let account = current_account(provider);
    let pricing = crate::pricing::Pricing::loaded().fingerprint();
    for w in &mut report.windows {
        w.machine_share = None;
        w.others_share = None;
        w.share_confidence = None;
        w.window_budget = None;
        let Some(account) = &account else {
            continue;
        };
        if w.trailing.as_deref() == Some("Off") || crate::usage::is_model_scoped_title(&w.title) {
            continue;
        }
        let cycle = cycle_secs(w);
        let series = hist
            .get(&key(provider, &w.title, account, cycle, &pricing))
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let reset = w.resets_at.as_deref().and_then(parse_rfc3339);
        if reset.is_none() || series.last().map(|s| s.reset_ts) != Some(reset) {
            continue;
        }
        if let Some(r) = estimate(series, now, w.utilization, cycle) {
            w.machine_share = Some(r.this_machine);
            w.others_share = Some(r.others);
            w.share_confidence = Some(r.confidence);
            w.window_budget = Some(r.q);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a series from cumulative increments (Δcost, Δu), 5 min apart, ending
    /// at `now`. Starts at (0,0).
    fn series(now: i64, incs: &[(f64, f64)]) -> Vec<ShareSample> {
        series_calib(now, incs, false)
    }

    fn series_calib(now: i64, incs: &[(f64, f64)], calibrated: bool) -> Vec<ShareSample> {
        let n = incs.len();
        let mut out = vec![ShareSample {
            reset_ts: None,
            ts: now - (n as i64) * 300,
            u: 0.0,
            local_cost: 0.0,
            calibrated,
        }];
        let (mut c, mut u) = (0.0, 0.0);
        for (i, (dc, du)) in incs.iter().enumerate() {
            c += dc;
            u += du;
            out.push(ShareSample {
                reset_ts: None,
                ts: now - (n as i64 - 1 - i as i64) * 300,
                u,
                local_cost: c,
                calibrated,
            });
        }
        out
    }

    #[test]
    fn delayed_quota_updates_include_idle_polls() {
        let now = 1_000_000;
        let mut incs = Vec::new();
        for _ in 0..4 {
            incs.extend([(10.0, 0.01), (0.0, 0.09)]);
        }
        let r = estimate(&series_calib(now, &incs, true), now, 0.40, 18_000).unwrap();
        assert!((r.q - 100.0).abs() < 1e-6);
        assert!((r.this_machine - 0.40).abs() < 1e-6);
        assert!(r.confidence < 0.35);
    }

    #[test]
    fn concurrent_devices_cannot_be_identified_from_a_stable_slope() {
        let now = 1_000_000;
        let r = estimate(&series(now, &[(10.0, 0.10); 5]), now, 0.50, 18_000).unwrap();
        assert!(r.confidence < 0.35);
    }

    #[test]
    fn stale_mismatched_saturated_and_invalid_snapshots_are_hidden() {
        let now = 1_000_000;
        let s = series_calib(now, &[(10.0, 0.10); 5], true);
        assert!(estimate(&s, now + MAX_SAMPLE_AGE + 1, 0.50, 604_800).is_none());
        assert!(estimate(&s, now, 0.10, 18_000).is_none());
        for u in [f64::NAN, f64::INFINITY, -0.1, 1.0, 1.1] {
            assert!(estimate(&s, now, u, 18_000).is_none());
        }
        assert!(estimate(&s, now - 1, 0.50, 18_000).is_none());
    }

    #[test]
    fn reset_boundaries_are_respected_and_legacy_toggle_is_ignored() {
        let now = 1_000_000;
        let mut s = series_calib(now, &[(10.0, 0.10); 5], true);
        for (i, sample) in s.iter_mut().enumerate() {
            sample.reset_ts = Some(now + 100 + i as i64);
        }
        assert!(estimate(&s, now, 0.50, 18_000).is_none());
        for (i, sample) in s.iter_mut().enumerate() {
            sample.reset_ts = None;
            sample.calibrated = i % 2 == 0;
        }
        assert!(estimate(&s, now, 0.50, 18_000).unwrap().confidence < 0.35);
    }

    #[test]
    fn history_is_isolated_by_account_duration_and_prices() {
        let a = key("claude", "Weekly", "a", 604800, "prices1");
        assert_ne!(a, key("claude", "Weekly", "b", 604800, "prices1"));
        assert_ne!(a, key("claude", "Weekly", "a", 604800, "prices2"));
        assert_ne!(a, key("claude", "Weekly", "a", 18000, "prices1"));
        // Preserve existing Claude observations, but never fit recovered
        // Codex request costs against pre-fix counter-only observations.
        assert_eq!(a, "v4/claude/a/604800/prices1/Weekly");
        assert_ne!(
            key("codex", "Weekly", "a", 604800, "prices1"),
            "v4/codex/a/604800/prices1/Weekly"
        );
    }

    #[test]
    fn inconsistent_calibration_stays_uncertain() {
        let now = 1_000_000;
        let s = series_calib(
            now,
            &[(5.0, 0.1), (50.0, 0.1), (10.0, 0.1), (100.0, 0.1)],
            true,
        );
        assert!(estimate(&s, now, 0.4, 18_000).unwrap().confidence < 0.35);
    }

    #[test]
    fn weighted_percentile_step() {
        let v = vec![(0.01, 1.0), (0.02, 1.0), (0.03, 1.0), (0.10, 1.0)];
        assert_eq!(weighted_percentile(&v, 0.20), Some(0.01));
        assert!(weighted_percentile(&[], 0.2).is_none());
    }

    #[test]
    fn window_secs_parsing() {
        assert_eq!(window_secs_for_title("5-Hour"), 18_000);
        assert_eq!(window_secs_for_title("2-Hour"), 7_200);
        assert_eq!(window_secs_for_title("Weekly"), 604_800);
        assert_eq!(window_secs_for_title("Weekly (Fable)"), 604_800);
        assert_eq!(window_secs_for_title("Monthly"), 2_592_000);
        assert_eq!(window_secs_for_title("Limit"), 18_000); // fallback
    }

    #[test]
    fn steady_single_device_attributes_all_to_this_machine() {
        let now = 1_000_000;
        // cost and U rise together: Q = $100 (each $10 → 0.10 of the window).
        let s = series_calib(now, &[(10.0, 0.10); 5], true);
        let r = estimate(&s, now, 0.50, 18_000).unwrap();
        assert!(
            (r.this_machine - 0.50).abs() < 0.02,
            "this={}",
            r.this_machine
        );
        assert!(r.others < 0.02, "others={}", r.others);
        assert!((r.q - 100.0).abs() < 5.0, "q={}", r.q);
        assert!(r.confidence < 0.35, "constant activity must stay uncertain");
    }

    #[test]
    fn other_devices_inflate_u_so_some_is_attributed_to_others() {
        let now = 1_000_000;
        // Mostly single-device intervals (slope 0.005 → Q=$200), with a couple
        // where another device also burned (slope 0.015). Floor → Q≈200.
        let s = series(
            now,
            &[
                (10.0, 0.05),
                (10.0, 0.05),
                (10.0, 0.15),
                (10.0, 0.05),
                (10.0, 0.15),
                (10.0, 0.05),
            ],
        );
        // cumulative cost = 60, cumulative U = 0.50.
        let r = estimate(&s, now, 0.50, 18_000).unwrap();
        assert!(r.others > 0.10, "others={}", r.others);
        assert!(r.this_machine < r.others + 0.25 && r.this_machine < 0.50);
        assert!((r.q - 200.0).abs() < 40.0, "q={}", r.q);
    }

    #[test]
    fn flat_window_has_no_confident_fit() {
        let now = 1_000_000;
        // U barely moves, cost barely moves → no qualifying slopes → cold-start
        // (hidden) at best.
        let s = series(now, &[(0.001, 0.0001); 4]);
        let r = estimate(&s, now, 0.001, 18_000);
        assert!(r.map_or(true, |r| r.confidence < 0.35));
    }

    #[test]
    fn automatic_fit_learns_without_a_user_toggle() {
        let now = 1_000_000;
        // Repeated natural idle / light / heavy usage, with another device
        // consuming a steady 1% per ten minutes. True budget is $200.
        let incs: Vec<_> = [0.0, 4.0, 12.0, 2.0, 0.0, 8.0, 16.0, 4.0]
            .into_iter()
            .flat_map(|c| [(c / 2.0, c / 400.0 + 0.005); 2])
            .collect();
        let s = series(now, &incs);
        let u = s.last().unwrap().u;
        let r = estimate(&s, now, u, 18_000).unwrap();
        assert!((r.q - 200.0).abs() < 1e-6);
        assert!((r.others - 0.08).abs() < 1e-6);
        assert!(r.confidence > 0.6);
    }

    #[test]
    fn automatic_fit_absorbs_delayed_updates_and_small_quota_rounding() {
        let now = 1_000_000;
        let mut incs = Vec::new();
        for (i, c) in [0.0, 4.0, 12.0, 2.0, 0.0, 8.0, 16.0, 4.0]
            .into_iter()
            .enumerate()
        {
            // Spend first, delayed quota on the next poll; ±0.1 percentage
            // point provider rounding on each whole block.
            let jitter: f64 = if i % 2 == 0 { 0.001 } else { -0.001 };
            incs.extend([(c, 0.0), (0.0, (c / 200.0 + jitter).max(0.0))]);
        }
        let s = series(now, &incs);
        let r = estimate(&s, now, s.last().unwrap().u, 18_000).unwrap();
        assert!((r.q / 200.0 - 1.0).abs() < 0.05);
        assert!(r.confidence > 0.6);
        assert!(r.others < 0.02);
    }

    #[test]
    fn gaps_do_not_manufacture_automatic_evidence() {
        let now = 1_000_000;
        let incs: Vec<_> = [0.0, 4.0, 12.0, 2.0, 0.0, 8.0, 16.0, 4.0]
            .into_iter()
            .flat_map(|c| [(c / 2.0, c / 400.0 + 0.005); 2])
            .collect();
        let mut s = series(now, &incs);
        let n = s.len();
        for (i, sample) in s.iter_mut().enumerate() {
            sample.ts = now - ((n - 1 - i) as i64) * (MAX_SAMPLE_GAP + 1);
        }
        assert!(estimate(&s, now, s.last().unwrap().u, 604800).is_none());
    }

    #[test]
    fn automatic_fit_rejects_changed_budget_and_unrelated_activity() {
        let now = 1_000_000;
        let costs = [0.0, 4.0, 12.0, 2.0, 0.0, 8.0, 16.0, 4.0];
        let incs: Vec<_> = costs
            .into_iter()
            .enumerate()
            .flat_map(|(i, c)| {
                let q = if i < 4 { 200.0 } else { 400.0 };
                [(c / 2.0, c / q / 2.0 + 0.005); 2]
            })
            .collect();
        let s = series(now, &incs);
        assert!(
            estimate(&s, now, s.last().unwrap().u, 18_000)
                .unwrap()
                .confidence
                < 0.35
        );
    }

    #[test]
    fn legacy_manual_assertion_cannot_force_confidence() {
        let now = 1_000_000;
        let incs = [(10.0, 0.10); 5];
        let a = estimate(&series_calib(now, &incs, false), now, 0.5, 18_000).unwrap();
        let b = estimate(&series_calib(now, &incs, true), now, 0.5, 18_000).unwrap();
        assert_eq!(a.confidence, b.confidence);
        assert!(b.confidence < 0.35);
    }

    /// The bug behind "this machine ~15% of a window it barely touched": a short
    /// burst here (Q=$300), then another device burns for hours while this
    /// machine is idle. Every poll during that stretch used to repeat the same
    /// depressed cumulative ratio; counted per sample they outnumbered the burst
    /// and pulled the fitted budget down to their level, inflating this
    /// machine's share several-fold. Idle polls now carry no weight at all.
    #[test]
    fn idle_polls_while_others_burn_do_not_drag_the_budget_down() {
        let now = 1_000_000;
        let mut incs = vec![(15.0, 0.05); 4]; // sole device: $60 → 20%
        incs.push((0.0, 0.10)); // another device takes 10% in one interval
        incs.extend(std::iter::repeat((0.0, 0.0)).take(60)); // 5h of idle polls
        incs.push((0.0, 0.10)); // …and another 10% from elsewhere
        let s = series(now, &incs);
        // u = 0.40 now; this machine's true share is $60 / $300 = 20%.
        let r = estimate(&s, now, 0.40, 604_800).unwrap();
        assert!((r.q - 300.0).abs() < 30.0, "q={}", r.q);
        assert!(
            (r.this_machine - 0.20).abs() < 0.03,
            "this={}",
            r.this_machine
        );
        assert!((r.others - 0.20).abs() < 0.03, "others={}", r.others);
    }

    /// A stretch where only other devices burn contributes no weight at all —
    /// even if it changes u every sample — so it can't outvote this machine's
    /// own evidence about the budget.
    #[test]
    fn others_only_stretch_adds_no_weight() {
        let now = 1_000_000;
        let mut incs = vec![(20.0, 0.10); 3]; // sole device: $60 → 30%, Q=$200
        incs.extend(std::iter::repeat((0.0, 0.01)).take(30)); // others: +30%
        let s = series(now, &incs);
        let r = estimate(&s, now, 0.60, 604_800).unwrap();
        assert!((r.q - 200.0).abs() < 20.0, "q={}", r.q);
        assert!(
            (r.this_machine - 0.30).abs() < 0.03,
            "this={}",
            r.this_machine
        );
        assert!((r.others - 0.30).abs() < 0.03, "others={}", r.others);
    }

    /// Another device goes *first* in the cycle (44% before this machine sends
    /// a single request), then this machine spends $90 for its own 18%. The old
    /// cumulative `cost / U` inherited the head start and could only ever read
    /// "this machine = everything"; interval ratios measure this machine's own
    /// slope and leave the 44% where it belongs.
    #[test]
    fn others_going_first_in_the_cycle_are_not_attributed_to_this_machine() {
        let now = 1_000_000;
        let mut incs = vec![(0.0, 0.11); 4]; // others: 44%, this machine idle
        incs.extend([(10.0, 0.02); 9]); // this machine: $90 → 18% (Q=$500)
        let s = series(now, &incs);
        let r = estimate(&s, now, 0.62, 604_800).unwrap();
        assert!((r.q - 500.0).abs() < 50.0, "q={}", r.q);
        assert!(
            (r.this_machine - 0.18).abs() < 0.03,
            "this={}",
            r.this_machine
        );
        assert!(r.others > 0.42, "others={}", r.others);
    }

    #[test]
    fn window_span_alignment() {
        let week = 604_800;
        let at = 1_000_000;
        // Reset ahead: the window started a week before it.
        assert_eq!(window_span(at + 100, week, at), Some((at + 100 - week, at)));
        // Reset implausibly far ahead: never a negative span.
        assert_eq!(window_span(at + 2 * week, week, at), None);
        // A passed reset cannot safely align the provider's reported usage.
        assert_eq!(window_span(at - 3600, week, at), None);
        // A whole cycle stale: unalignable.
        assert_eq!(window_span(at - week, week, at), None);
    }

    #[test]
    fn reset_drop_does_not_make_a_negative_slope() {
        let now = 1_000_000;
        // Two cycles separated by a reset (U and cost drop back to ~0).
        let mut s = series(now - 3_000, &[(10.0, 0.10); 3]); // cycle 1
                                                             // cycle 2 after a reset
        let c2 = series(now, &[(10.0, 0.10); 3]);
        s.extend(c2);
        let r = estimate(&s, now, 0.30, 18_000).unwrap();
        // Still a clean Q≈100 fit; no panic / no negative attribution.
        assert!(r.this_machine >= 0.0 && r.others >= 0.0);
        assert!((r.q - 100.0).abs() < 10.0, "q={}", r.q);
    }
}
