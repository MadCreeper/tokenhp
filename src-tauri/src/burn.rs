//! Burn-rate projection: "at your recent pace, will this quota window run out
//! before it resets?" — the one thing the live percentages don't tell you.
//!
//! Pure math over a short history of `(timestamp, used-fraction)` samples (the
//! `ambient` poll records them). Deliberately conservative: it only projects a
//! limit-hit when there's been a *sustained* burn over a meaningful span, so it
//! stays quiet while you're idle or coasting and can't flap on a single busy
//! minute. The caller decides to surface it only when the projected empty-time
//! lands *before* the window's reset (see `ambient::annotate`).

/// One quota observation: `used` is the consumed fraction (0..1) at unix `ts`.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct Sample {
    pub ts: i64,
    pub used: f64,
}

/// Don't look back further than this for the burn baseline (seconds). Older
/// activity isn't representative of the current session's pace.
pub const LOOKBACK_SECS: i64 = 90 * 60;
/// Need at least this much elapsed between baseline and now, or the rate is just
/// noise (seconds).
pub const MIN_SPAN_SECS: i64 = 25 * 60;
/// Need at least this much extra consumption over the span, else it's a flat/idle
/// window and projecting is meaningless.
pub const MIN_DELTA: f64 = 0.03;

/// Seconds until `used` reaches 1.0 at the burn rate measured from the oldest
/// in-window baseline to now, or `None` when there isn't enough sustained burn to
/// say. `now_used` is the latest consumed fraction; `samples` is the prior
/// history (any order), each strictly before `now_ts`.
pub fn eta_to_empty(samples: &[Sample], now_ts: i64, now_used: f64) -> Option<i64> {
    if now_used >= 1.0 {
        return Some(0);
    }
    // Baseline = the OLDEST sample within the lookback window that isn't already
    // past `now_used` (a higher earlier reading means the window reset in between,
    // so it can't anchor a forward rate).
    let baseline = samples
        .iter()
        .filter(|s| {
            let age = now_ts - s.ts;
            age > 0 && age <= LOOKBACK_SECS && s.used <= now_used
        })
        .min_by_key(|s| s.ts)?;

    let span = now_ts - baseline.ts;
    let delta = now_used - baseline.used;
    if span < MIN_SPAN_SECS || delta < MIN_DELTA {
        return None;
    }

    let rate_per_sec = delta / span as f64; // fraction consumed per second
    if rate_per_sec <= 0.0 {
        return None;
    }
    let secs = ((1.0 - now_used) / rate_per_sec).round();
    if secs.is_finite() && secs >= 0.0 {
        Some(secs as i64)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(ts: i64, used: f64) -> Sample {
        Sample { ts, used }
    }

    #[test]
    fn steady_burn_projects_linearly() {
        // 0.50 → 0.60 over 30 min ⇒ 0.10 / 1800s. 0.40 remaining ⇒ 7200s (2h).
        let now = 100_000;
        let hist = [s(now - 1800, 0.50)];
        assert_eq!(eta_to_empty(&hist, now, 0.60), Some(7200));
    }

    #[test]
    fn picks_oldest_in_lookback_as_baseline() {
        let now = 100_000;
        let hist = [s(now - 1800, 0.55), s(now - 3600, 0.50), s(now - 600, 0.58)];
        // Oldest within lookback is t-3600 @0.50; 0.10 over 3600 ⇒ 0.40/(0.10/3600)=14400.
        assert_eq!(eta_to_empty(&hist, now, 0.60), Some(14400));
    }

    #[test]
    fn too_short_a_span_is_none() {
        let now = 100_000;
        // Only a 5-min span — below MIN_SPAN.
        let hist = [s(now - 300, 0.50)];
        assert_eq!(eta_to_empty(&hist, now, 0.70), None);
    }

    #[test]
    fn flat_window_is_none() {
        let now = 100_000;
        let hist = [s(now - 3600, 0.599)]; // <MIN_DELTA growth
        assert_eq!(eta_to_empty(&hist, now, 0.60), None);
    }

    #[test]
    fn reset_in_history_is_ignored() {
        let now = 100_000;
        // An old high reading (0.9) is from before a reset; only the post-reset
        // low baseline (0.20) should anchor the rate.
        let hist = [s(now - 5000, 0.90), s(now - 1800, 0.20)];
        // 0.20 → 0.35 over 1800 ⇒ 0.15/1800; remaining 0.65 ⇒ 7800s.
        assert_eq!(eta_to_empty(&hist, now, 0.35), Some(7800));
    }

    #[test]
    fn beyond_lookback_only_is_none() {
        let now = 100_000;
        let hist = [s(now - (LOOKBACK_SECS + 600), 0.30)];
        assert_eq!(eta_to_empty(&hist, now, 0.80), None);
    }

    #[test]
    fn already_empty_is_zero() {
        assert_eq!(eta_to_empty(&[], 100_000, 1.0), Some(0));
    }
}
