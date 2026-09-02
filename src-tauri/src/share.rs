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
//! We estimate the one scalar `Q` per (provider, window). Both Claude and Codex
//! meter by model-weighted token cost, so local **dollar cost** (priced via
//! `pricing.json`) is the bridge; `Q` absorbs the unknown credit↔dollar factor.
//!
//! Key trick (the "automatic calibration"): other devices can only *add* to `U`,
//! so over any interval the ratio `r = Δlocal_cost / ΔU` is always **≤ Q**
//! (others inflate the denominator). Its **high percentile** = `Q` — captured
//! from the intervals where this machine was the sole/dominant contributor, with
//! no need to know when those were. Recency weighting lets `Q` re-learn after a
//! limit change or free reset.
//!
//! Which intervals, and how they're weighted, is what makes this hold up:
//!
//! - **Only this machine's active intervals count**, and each ratio is weighted
//!   by this machine's spend in it. A poll where this machine spent nothing says
//!   nothing about `Q`; whatever `U` did meanwhile was other devices (excluded
//!   from the denominator entirely). Counting one ratio per poll instead — the
//!   original design, over the cumulative `cost / U` since the window began —
//!   let a long idle stretch after another device had burned repeat one
//!   depressed ratio hundreds of times and drag `Q` down to it, which surfaced
//!   as a confident "this machine ~15%" on a window this machine had barely
//!   touched. The cumulative form also sank for the rest of a cycle whenever
//!   another device went *first*: every later ratio inherited that head start.
//! - **Intervals are merged until `U` has moved by [`MIN_DU`]** so the 1%
//!   quantisation of the provider's figure stays a bounded error rather than
//!   noise that swamps the slope.

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
    /// Recorded while the user asserted "I'm the only active device" — these
    /// intervals are trusted as the single-device floor (weighted up in the fit).
    /// `#[serde(default)]` so samples written before this field still load.
    #[serde(default)]
    pub calibrated: bool,
}

/// Keep ~16 days so the weekly fit always has a baseline; the file persists
/// across restarts (the weekly/monthly fits need days of history).
const SHARE_RETAIN_SECS: i64 = 16 * 86_400;
/// Beyond this age, thin to one sample per `THIN_BUCKET_SECS` to bound file size.
const RECENT_FULL_SECS: i64 = 2 * 3600;
const THIN_BUCKET_SECS: i64 = 15 * 60;
/// High percentile of the interval ratio `Δlocal_cost / ΔU` = the window budget
/// `Q` (sole-device-dominant intervals sit near it; others-active ones below).
const RATIO_PCTILE: f64 = 0.80;
/// An interval must move `U` by at least this much before it yields a ratio;
/// smaller moves are merged into the next. The provider quantises `U` to 1%, so
/// this bounds a ratio's quantisation error to ±20%.
const MIN_DU: f64 = 0.05;
/// Need at least this many usable ratio samples to attempt a fit.
const MIN_RATIO_SAMPLES: usize = 3;
/// Ratios within ±this of the fitted `Q` count as sole-device evidence *for* it.
const SUPPORT_TOL: f64 = 0.25;
/// Confidence saturates once this machine's (recency-weighted) spend observed at
/// ratios consistent with `Q` reaches this fraction of the budget — i.e. this
/// machine has itself moved the window by that much, well clear of the 1%
/// quantisation of the provider's utilization figure.
const CONF_TARGET_FRAC: f64 = 0.25;
/// Weight multiplier for samples the user marked sole-device (calibration).
const CALIB_BOOST: f64 = 4.0;

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
    let current_c = samples.last()?.local_cost;

    // Implied-budget ratios r = Δlocal_cost / ΔU over this machine's active
    // intervals (see the module docs). Each is ≤ the true Q — other devices can
    // only inflate ΔU — so the high percentile is the sole-device-dominant
    // budget. Weighted by this machine's spend in the interval, times recency,
    // times the calibration boost for samples the user asserted sole-device.
    let mut ratios: Vec<(f64, f64)> = Vec::new(); // (r, weight)

    // Consecutive active intervals accumulate here until U has moved enough
    // (MIN_DU) for their ratio to be worth emitting.
    let (mut acc_c, mut acc_u) = (0.0, 0.0);
    let mut prev: Option<&ShareSample> = None;
    for s in samples {
        let Some(p) = prev else {
            prev = Some(s);
            continue;
        };
        prev = Some(s);
        // Either figure falling back means the window reset (or was
        // re-aligned) since the last sample: nothing spans that boundary.
        if s.local_cost + 1e-9 < p.local_cost || s.u + 1e-9 < p.u {
            acc_c = 0.0;
            acc_u = 0.0;
            continue;
        }
        let dc = s.local_cost - p.local_cost;
        if dc <= 1e-9 {
            // This machine spent nothing here. Whatever U did was other devices
            // (or the provider registering earlier spend late): leave it out of
            // the denominator rather than let it depress the running ratio.
            continue;
        }
        acc_c += dc;
        acc_u += s.u - p.u;
        // Epsilon: sums of 1%-quantised deltas land at 0.04999… as easily as
        // 0.05000…, and a ratio held back by that merges a clean stretch into
        // whatever contaminated one follows.
        if acc_u + 1e-9 < MIN_DU {
            continue;
        }
        let r = acc_c / acc_u;
        let recency = 0.5_f64.powf((now - s.ts) as f64 / cycle_secs as f64);
        let calib = if s.calibrated { CALIB_BOOST } else { 1.0 };
        ratios.push((r, acc_c * recency * calib));
        acc_c = 0.0;
        acc_u = 0.0;
    }

    let mut q = f64::NAN;
    let mut confidence = 0.0;
    if ratios.len() >= MIN_RATIO_SAMPLES {
        if let Some(qq) = weighted_percentile(&ratios, RATIO_PCTILE) {
            if qq > 0.0 {
                q = qq;
                // Support: how much of this machine's (recency-weighted) spend
                // was observed at ratios consistent with Q — the sole-device
                // evidence the fit actually rests on — as a fraction of the
                // budget. Others-active samples sit far below Q; they neither
                // support nor (as the old "is the ratio steady" score did)
                // penalise it, since a depressed-but-steady ratio is exactly
                // what another device's usage looks like.
                let support: f64 = ratios
                    .iter()
                    .filter(|(r, _)| {
                        *r >= qq * (1.0 - SUPPORT_TOL) && *r <= qq * (1.0 + SUPPORT_TOL)
                    })
                    .map(|(_, w)| w)
                    .sum();
                let c_support = (support / qq / CONF_TARGET_FRAC).clamp(0.0, 1.0);
                let latest = samples.last().map(|s| s.ts).unwrap_or(now);
                let c_rec = 0.5_f64.powf((now - latest) as f64 / cycle_secs as f64);
                confidence = (c_support * c_rec).clamp(0.0, 1.0);
            }
        }
    }
    if !(q.is_finite() && q > 0.0) {
        // Cold start: too little data. Assume sole device (Q = C/U) but keep
        // confidence low so the UI stays hidden until samples accumulate.
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
    // v1 Codex samples stamped a cached rollout snapshot with every poll's
    // wall-clock time and paired it with cost ending at that later time. They
    // cannot be repaired from the persisted fields alone; worse, their future
    // timestamps can make `record_at` reject the first correctly stamped
    // snapshot after an upgrade. Start Codex's fit clean so the UI says
    // "estimating" rather than carrying a confident false split forward.
    if provider == "codex" {
        format!("codex-v2/{title}")
    } else {
        format!("{provider}/{title}")
    }
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

/// The local time span whose cost pairs with a utilization figure reported at
/// `at`, for a window of `window_secs` whose reset instant is `reset_ts`:
///
/// - reset still ahead: the window is `[reset − length, at]`;
/// - reset already passed: the provider hasn't published the new window's clock
///   yet (Claude keeps reporting the old `resets_at` for a while after a reset;
///   a Codex rollout snapshot is only as fresh as the last turn). Whatever the
///   new window holds was spent *after* the old reset, so count from there.
///   Sliding a whole window back from a stale reset — the old behaviour —
///   dragged the previous cycle's spend into the new one, which is how a window
///   this machine hadn't touched since the reset came out at "this machine 15%";
/// - a whole window or more past the reset: a cycle stale, unalignable → `None`.
pub fn window_span(reset_ts: i64, window_secs: i64, at: i64) -> Option<(i64, i64)> {
    if reset_ts > at {
        Some(((reset_ts - window_secs).min(at - 1), at))
    } else if at - reset_ts < window_secs {
        Some((reset_ts, at))
    } else {
        None
    }
}

/// This machine's cost over the span the provider's window covers as of `at`
/// (see [`window_span`]). `None` when the window can't be aligned. Scans logs →
/// call off the UI thread. Excludes API-key tools (they don't consume the
/// subscription).
fn local_cost_in_window(
    provider: &str,
    resets_at: Option<&str>,
    title: &str,
    at: i64,
) -> Option<f64> {
    let window_secs = window_secs_for_title(title);
    let reset_ts = resets_at.and_then(parse_rfc3339)?;
    let (start, end) = window_span(reset_ts, window_secs, at)?;
    let cost = match provider {
        "codex" => crate::codexstats::collect_local_range(start, end)
            .iter()
            .filter_map(|m| m.cost.as_ref())
            .map(|c| c.total)
            .sum(),
        _ => crate::localstats::collect_range(start, end)
            .iter()
            .filter(|m| m.id.starts_with("claude"))
            .filter_map(|m| m.cost.as_ref())
            .map(|c| c.total)
            .sum(),
    };
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
    // Are we in a user-asserted "only active device" window? Those samples are
    // trusted as the single-device floor by the estimator.
    let calibrated = crate::ambient::load_settings(app).only_active_device;
    for w in &report.windows {
        if w.trailing.as_deref() == Some("Off") || w.resets_at.is_none() {
            continue;
        }
        // Per-model windows can't be fitted: the local-cost series is
        // account-wide, so their implied-budget ratio would be inflated.
        if crate::usage::is_model_scoped_title(&w.title) {
            continue;
        }
        let k = key(provider, &w.title);
        if hist
            .get(&k)
            .and_then(|v| v.last())
            .is_some_and(|last| at <= last.ts)
        {
            continue; // already observed this instant (or something newer)
        }
        let Some(cost) = local_cost_in_window(provider, w.resets_at.as_deref(), &w.title, at)
        else {
            continue;
        };
        hist.entry(k).or_default().push(ShareSample {
            ts: at,
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
    for w in &mut report.windows {
        if w.trailing.as_deref() == Some("Off") || crate::usage::is_model_scoped_title(&w.title) {
            continue;
        }
        let series = hist
            .get(&key(provider, &w.title))
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let cycle = window_secs_for_title(&w.title);
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
                ts: now - (n as i64 - 1 - i as i64) * 300,
                u,
                local_cost: c,
                calibrated,
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
        assert_eq!(window_secs_for_title("Weekly (Fable)"), 604_800);
        assert_eq!(window_secs_for_title("Monthly"), 2_592_000);
        assert_eq!(window_secs_for_title("Limit"), 18_000); // fallback
    }

    #[test]
    fn legacy_codex_history_is_not_reused() {
        assert_eq!(key("codex", "Weekly"), "codex-v2/Weekly");
        assert_eq!(key("claude", "Weekly"), "claude/Weekly");
    }

    #[test]
    fn steady_single_device_attributes_all_to_this_machine() {
        let now = 1_000_000;
        // cost and U rise together: Q = $100 (each $10 → 0.10 of the window).
        let s = series(now, &[(10.0, 0.10); 5]);
        let r = estimate(&s, now, 0.50, 18_000).unwrap();
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
    fn calibration_boosts_confidence_for_sole_device() {
        let now = 1_000_000;
        // A short sole-device run (Q≈100) that has only moved the window 18% —
        // not yet enough spend for full confidence on its own.
        let incs = [(3.0, 0.03); 6];
        let plain = estimate(&series_calib(now, &incs, false), now, 0.18, 18_000).unwrap();
        let calib = estimate(&series_calib(now, &incs, true), now, 0.18, 18_000).unwrap();
        assert!((calib.this_machine - 0.18).abs() < 0.02);
        assert!(
            calib.confidence > plain.confidence,
            "calib {} should exceed plain {}",
            calib.confidence,
            plain.confidence
        );
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
        assert!((r.this_machine - 0.20).abs() < 0.03, "this={}", r.this_machine);
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
        assert!((r.this_machine - 0.30).abs() < 0.03, "this={}", r.this_machine);
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
        assert!((r.this_machine - 0.18).abs() < 0.03, "this={}", r.this_machine);
        assert!(r.others > 0.42, "others={}", r.others);
    }

    #[test]
    fn window_span_alignment() {
        let week = 604_800;
        let at = 1_000_000;
        // Reset ahead: the window started a week before it.
        assert_eq!(window_span(at + 100, week, at), Some((at + 100 - week, at)));
        // Reset implausibly far ahead: never a negative span.
        assert_eq!(window_span(at + 2 * week, week, at), Some((at - 1, at)));
        // Reset passed but the clock not yet republished: count from the reset,
        // not a whole window back from it (which would re-count the old cycle).
        assert_eq!(window_span(at - 3600, week, at), Some((at - 3600, at)));
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
