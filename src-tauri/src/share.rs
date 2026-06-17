//! "What % of the plan is this machine using?" — fits the account-wide
//! utilization the provider reports against this machine's local cost, to split
//! each window into **this machine** vs **other devices**.
//!
//! The provider's API/local data gives account-wide `U` (0..1) per window; the
//! local logs only see this machine. There is no device id, so other devices are
//! *inferred*: for a window with budget `Q` (the dollar-cost that equals 100%),
//!
//!     U ≈ (cost_this_machine + cost_other_devices) / Q
//!     this_machine = cost_this_machine / Q ,   others = U − this_machine
//!
//! We estimate the one scalar `Q` per (provider, window). Both Claude and Codex
//! meter by model-weighted token cost, so local **dollar cost** (priced via
//! `pricing.json`) is the bridge; `Q` absorbs the unknown credit↔dollar factor.
//!
//! Key trick (the "automatic calibration"): other devices can only *add* to `U`,
//! so over short intervals `ΔU/Δcost` has a **lower envelope** = the single-device
//! rate `1/Q`. A recency-weighted low percentile of those slopes finds the floor
//! without needing to know when you were the only active device. Recency
//! weighting lets it re-learn `Q` after a limit change or a free reset.

use crate::usage::UsageReport;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::{AppHandle, Manager};

/// One observation for a (provider, window): account-wide `u` (0..1) and this
/// machine's cumulative cost since the window's last reset, at unix `ts`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ShareSample {
    pub ts: i64,
    pub u: f64,
    pub local_cost: f64,
}

/// Keep ~16 days so the weekly fit always has a baseline; the file persists
/// across restarts (the weekly/monthly fits need days of history).
const SHARE_RETAIN_SECS: i64 = 16 * 86_400;
/// Beyond this age, thin to one sample per `THIN_BUCKET_SECS` to bound file size.
const RECENT_FULL_SECS: i64 = 2 * 3600;
const THIN_BUCKET_SECS: i64 = 15 * 60;
/// Low percentile of `ΔU/Δcost` slopes = the single-device floor (1/Q).
const FLOOR_PCTILE: f64 = 0.20;
/// Effective sample count at which `c_count` saturates.
const CONF_TARGET_N: f64 = 8.0;

// ---- window descriptors -----------------------------------------------------

/// Seconds spanned by a window title: "5-Hour"/"N-Hour" → N·3600, "Weekly" → 7d,
/// "Monthly" → 30d; default 5h. Used to align local cost to the server window and
/// as the recency half-life (one cycle).
pub fn window_secs_for_title(title: &str) -> i64 {
    if let Some(h) = title.strip_suffix("-Hour") {
        if let Ok(n) = h.trim().parse::<i64>() {
            return (n.max(1)) * 3600;
        }
    }
    match title {
        "Weekly" => 7 * 86_400,
        "Monthly" => 30 * 86_400,
        _ => 5 * 3600,
    }
}

/// Minimum cost increment for an interval to count toward the fit — filters out
/// quantization-noise-dominated tiny intervals. Scales with the window.
fn min_dc_for_title(title: &str) -> f64 {
    match window_secs_for_title(title) {
        s if s <= 6 * 3600 => 0.02,
        s if s <= 7 * 86_400 => 0.10,
        _ => 0.25,
    }
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
/// utilization `live_u`. `cycle_secs` = window length (recency half-life);
/// `min_dc` = minimum qualifying cost increment. Returns `None` when there's no
/// usable data at all.
pub fn estimate(
    samples: &[ShareSample],
    now: i64,
    live_u: f64,
    cycle_secs: i64,
    min_dc: f64,
) -> Option<ShareResult> {
    let current_c = samples.last()?.local_cost;

    // Auto-detect utilization resolution = smallest positive within-segment ΔU.
    let mut min_pos_du = f64::INFINITY;
    for w in samples.windows(2) {
        let du = w[1].u - w[0].u;
        if du > 1e-9 {
            min_pos_du = min_pos_du.min(du);
        }
    }
    let resolution = if min_pos_du.is_finite() {
        min_pos_du.clamp(0.002, 0.02)
    } else {
        0.01
    };
    let min_du = 2.0 * resolution;

    // Build coalesced, recency-weighted slopes ΔU/Δcost (the lower-envelope set).
    let mut slopes: Vec<(f64, f64)> = Vec::new(); // (slope, weight)
    let mut latest_slope_ts = i64::MIN;
    let (mut acc_du, mut acc_dc) = (0.0_f64, 0.0_f64);
    for w in samples.windows(2) {
        let du = w[1].u - w[0].u;
        let dc = w[1].local_cost - w[0].local_cost;
        // A drop in either signals a reset (scheduled or free): discard the
        // partial accumulator and don't form a slope across the boundary.
        if du < -1e-9 || dc < -1e-9 {
            acc_du = 0.0;
            acc_dc = 0.0;
            continue;
        }
        acc_du += du;
        acc_dc += dc;
        if acc_du >= min_du && acc_dc >= min_dc {
            let slope = acc_du / acc_dc;
            let ts = w[1].ts;
            let recency = 0.5_f64.powf((now - ts) as f64 / cycle_secs as f64);
            let quant = (acc_du / (4.0 * resolution)).clamp(0.0, 1.0);
            let weight = recency * quant;
            if weight > 0.0 && slope.is_finite() && slope > 0.0 {
                slopes.push((slope, weight));
                latest_slope_ts = latest_slope_ts.max(ts);
            }
            acc_du = 0.0;
            acc_dc = 0.0;
        }
    }

    // Estimate Q from the slope floor; else cold-start prior (assume sole device).
    let mut q = f64::NAN;
    let mut confidence = 0.0;
    if let Some(floor) = weighted_percentile(&slopes, FLOOR_PCTILE) {
        if floor > 0.0 {
            q = 1.0 / floor;
            let n_eff: f64 = slopes.iter().map(|(_, w)| w).sum();
            let c_count = (n_eff / CONF_TARGET_N).clamp(0.0, 1.0);
            // c_fit: how tight is the floor (q20 vs q40)?
            let q20 = floor;
            let q40 = weighted_percentile(&slopes, 0.40).unwrap_or(q20);
            let c_fit = if q20 > 0.0 {
                (1.0 - (q40 / q20 - 1.0) / 0.5).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let c_rec = 0.5_f64.powf((now - latest_slope_ts) as f64 / cycle_secs as f64);
            confidence = (c_count * c_fit * c_rec).clamp(0.0, 1.0);
        }
    }
    if !(q.is_finite() && q > 0.0) {
        // Cold start: no usable slopes yet. Assume sole device (Q = C/U) but keep
        // confidence low so the UI stays hidden until real intervals accumulate.
        if live_u > 0.01 && current_c > 0.0 {
            q = current_c / live_u;
            confidence = 0.15;
        } else {
            return None;
        }
    }

    let raw_this = current_c / q;
    let this_machine = raw_this.clamp(0.0, live_u);
    let others = (live_u - this_machine).clamp(0.0, 1.0);
    Some(ShareResult {
        this_machine,
        others,
        q,
        confidence,
    })
}

/// Weighted step percentile of `(value, weight)` pairs (value ascending).
fn weighted_percentile(pairs: &[(f64, f64)], p: f64) -> Option<f64> {
    let mut v: Vec<(f64, f64)> = pairs.iter().copied().filter(|(_, w)| *w > 0.0).collect();
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

fn key(provider: &str, title: &str) -> String {
    format!("{provider}/{title}")
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

/// Drop samples past the retention horizon, then thin everything older than the
/// recent window to one per bucket (keeps the file small for weekly/monthly).
fn prune_and_thin(v: &mut Vec<ShareSample>, now: i64) {
    v.retain(|s| now - s.ts <= SHARE_RETAIN_SECS);
    let mut out: Vec<ShareSample> = Vec::with_capacity(v.len());
    let mut last_bucket = i64::MIN;
    for s in v.iter() {
        if now - s.ts <= RECENT_FULL_SECS {
            out.push(*s);
        } else {
            let bucket = s.ts / THIN_BUCKET_SECS;
            if bucket != last_bucket {
                out.push(*s);
                last_bucket = bucket;
            }
        }
    }
    *v = out;
}

/// This machine's cost over the same span the server window covers — aligned via
/// `resets_at` (window_start = reset − window length). Scans logs → call off the
/// UI thread. Excludes API-key tools (they don't consume the subscription).
fn local_cost_in_window(provider: &str, resets_at: Option<&str>, title: &str) -> f64 {
    let window_secs = window_secs_for_title(title);
    let now = now_unix();
    let span = match resets_at.and_then(parse_rfc3339) {
        Some(reset_ts) => (now - (reset_ts - window_secs)).clamp(1, window_secs),
        None => window_secs,
    };
    match provider {
        "codex" => crate::codexstats::collect_local(span)
            .iter()
            .filter_map(|m| m.cost.as_ref())
            .map(|c| c.total)
            .sum(),
        _ => crate::localstats::collect(span)
            .iter()
            .filter(|m| m.id.starts_with("claude"))
            .filter_map(|m| m.cost.as_ref())
            .map(|c| c.total)
            .sum(),
    }
}

/// Append a sample per resettable window. Scans logs (heavy) — caller should run
/// it on a blocking thread.
pub fn record(app: &AppHandle, provider: &str, report: &UsageReport) {
    let mut hist = load_history(app);
    let now = now_unix();
    for w in &report.windows {
        if w.trailing.as_deref() == Some("Off") || w.resets_at.is_none() {
            continue;
        }
        let cost = local_cost_in_window(provider, w.resets_at.as_deref(), &w.title);
        let v = hist.entry(key(provider, &w.title)).or_default();
        v.push(ShareSample {
            ts: now,
            u: w.utilization,
            local_cost: cost,
        });
        prune_and_thin(v, now);
    }
    save_history(app, &hist);
}

/// Fill each window's share fields from the recorded series. Light (reads the
/// small history file + pure math; no log scan) — safe on the command path.
pub fn annotate(app: &AppHandle, provider: &str, report: &mut UsageReport) {
    let hist = load_history(app);
    let now = now_unix();
    for w in &mut report.windows {
        if w.trailing.as_deref() == Some("Off") {
            continue;
        }
        let series = hist
            .get(&key(provider, &w.title))
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let cycle = window_secs_for_title(&w.title);
        let min_dc = min_dc_for_title(&w.title);
        if let Some(r) = estimate(series, now, w.utilization, cycle, min_dc) {
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
        let n = incs.len();
        let mut out = vec![ShareSample {
            ts: now - (n as i64) * 300,
            u: 0.0,
            local_cost: 0.0,
        }];
        let (mut c, mut u) = (0.0, 0.0);
        for (i, (dc, du)) in incs.iter().enumerate() {
            c += dc;
            u += du;
            out.push(ShareSample {
                ts: now - (n as i64 - 1 - i as i64) * 300,
                u,
                local_cost: c,
            });
        }
        out
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
        assert_eq!(window_secs_for_title("Monthly"), 2_592_000);
        assert_eq!(window_secs_for_title("Limit"), 18_000); // fallback
    }

    #[test]
    fn steady_single_device_attributes_all_to_this_machine() {
        let now = 1_000_000;
        // cost and U rise together: Q = $100 (each $10 → 0.10 of the window).
        let s = series(now, &[(10.0, 0.10); 5]);
        let r = estimate(&s, now, 0.50, 18_000, 0.02).unwrap();
        assert!((r.this_machine - 0.50).abs() < 0.02, "this={}", r.this_machine);
        assert!(r.others < 0.02, "others={}", r.others);
        assert!((r.q - 100.0).abs() < 5.0, "q={}", r.q);
        assert!(r.confidence > 0.35, "conf={}", r.confidence);
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
        let r = estimate(&s, now, 0.50, 18_000, 0.02).unwrap();
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
        let r = estimate(&s, now, 0.001, 18_000, 0.02);
        assert!(r.map_or(true, |r| r.confidence < 0.35));
    }

    #[test]
    fn reset_drop_does_not_make_a_negative_slope() {
        let now = 1_000_000;
        // Two cycles separated by a reset (U and cost drop back to ~0).
        let mut s = series(now - 3_000, &[(10.0, 0.10); 3]); // cycle 1
        // cycle 2 after a reset
        let c2 = series(now, &[(10.0, 0.10); 3]);
        s.extend(c2);
        let r = estimate(&s, now, 0.30, 18_000, 0.02).unwrap();
        // Still a clean Q≈100 fit; no panic / no negative attribution.
        assert!(r.this_machine >= 0.0 && r.others >= 0.0);
        assert!((r.q - 100.0).abs() < 10.0, "q={}", r.q);
    }
}
