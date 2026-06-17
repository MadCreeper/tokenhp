//! Ambient HP — makes the menu-bar icon a *live* gauge and adds proactive
//! low-quota alerts.
//!
//! HPBar's pitch is "your usage as a health bar in the menu bar", but until now
//! the tray icon was static — you had to open the popover to learn anything. A
//! small background task polls the live quota and:
//!   1. redraws the tray heart to your most-depleted window's remaining fraction
//!      (see [`crate::heart_icon`]), tinted green→red by danger,
//!   2. keeps the tray tooltip showing the exact percentages, and
//!   3. fires a native notification when a window crosses into "low" / "critical"
//!      (once per depletion, opt-out via the tray menu).
//!
//! It reuses the same credential cache and `/api/oauth/usage` fetch the popover
//! uses, so it adds no new auth and degrades gracefully (icon left untouched)
//! when you're not signed in.

use crate::credentials::CredentialCache;
use crate::heart_icon;
use crate::usage::{self, UsageReport};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use tauri::image::Image;
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

/// Tray icon id — must match the `TrayIconBuilder::with_id` in `lib.rs`.
pub const TRAY_ID: &str = "main";

/// How often the backend re-checks live quota to update the icon. Faster than the
/// popover's 30-min poll (the 5-hour window moves quickly and the icon is meant
/// to be glanceable) but still gentle on the endpoint.
const POLL_SECS: u64 = 300;
/// A short settle delay before the first poll so startup isn't competing with the
/// first popover render / tray construction.
const STARTUP_DELAY_SECS: u64 = 4;

/// Heart render scale → a 42×42 icon the OS downsamples to the bar height.
const SCALE: u32 = 6;

// --- alert severity ---------------------------------------------------------

/// Remaining-fraction at or below which a window is "low" (amber/orange).
const LOW: f64 = 0.20;
/// Remaining-fraction at or below which a window is "critical" (red).
const CRITICAL: f64 = 0.05;

/// Severity bucket for a remaining fraction. Higher = worse.
fn severity(remaining: f64) -> u8 {
    if remaining <= CRITICAL {
        2
    } else if remaining <= LOW {
        1
    } else {
        0
    }
}

// --- HP + tooltip from a usage report (pure, unit-tested) -------------------

/// The most-depleted *active* window's remaining fraction — the "are you about to
/// be blocked" signal that drives the icon. Disabled extra-usage ("Off") windows
/// are skipped. `None` when there are no windows.
pub fn min_remaining(report: &UsageReport) -> Option<f64> {
    report
        .windows
        .iter()
        .filter(|w| w.trailing.as_deref() != Some("Off"))
        .map(|w| w.remaining)
        .fold(None, |acc, r| Some(acc.map_or(r, |a: f64| a.min(r))))
}

/// Compact hover text, e.g. `HPBar · 5-Hour 47% · Weekly 80%`.
pub fn tooltip(report: &UsageReport) -> String {
    let parts: Vec<String> = report
        .windows
        .iter()
        .map(|w| {
            if w.trailing.as_deref() == Some("Off") {
                format!("{} off", w.title)
            } else {
                format!("{} {}%", w.title, pct(w.remaining))
            }
        })
        .collect();
    if parts.is_empty() {
        "HPBar".into()
    } else {
        format!("HPBar · {}", parts.join(" · "))
    }
}

fn pct(remaining: f64) -> i64 {
    (remaining * 100.0).round() as i64
}

// --- persisted settings (just the alert toggle, for now) --------------------

fn default_alerts() -> bool {
    true
}
fn default_theme() -> String {
    "minecraft".into()
}

#[derive(Serialize, Deserialize, Clone)]
pub struct AmbientSettings {
    /// Whether low/critical quota notifications fire. Toggle in the tray menu.
    #[serde(default = "default_alerts")]
    pub alerts_enabled: bool,
    /// Tray-heart theme id, mirrored from the popover (`src/theme.ts`). `#[serde
    /// (default)]` so configs written before this field still load.
    #[serde(default = "default_theme")]
    pub theme: String,
}

impl Default for AmbientSettings {
    fn default() -> Self {
        Self {
            alerts_enabled: default_alerts(),
            theme: default_theme(),
        }
    }
}

/// Last painted HP + tooltip, so a theme switch can repaint the tray instantly
/// from the new palette without waiting for (or forcing) a fresh network poll.
#[derive(Default)]
pub struct TrayState(pub std::sync::Mutex<Option<(f64, String)>>);

/// Persist the chosen tray theme and repaint the heart now with the last-known
/// HP (no network — the next poll refreshes the level). Frontend calls this when
/// the user switches theme.
pub fn set_theme_and_repaint(app: &AppHandle, theme: String) {
    let mut s = load_settings(app);
    s.theme = theme.clone();
    save_settings(app, &s);
    if let Some(st) = app.try_state::<TrayState>() {
        let snap = st.0.lock().unwrap().clone();
        if let Some((remaining, tip)) = snap {
            paint(app, remaining, &tip, heart_icon::TrayTheme::from_id(&theme));
        }
    }
}

fn settings_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    let dir = app.path().app_config_dir().ok()?;
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("ambient.json"))
}

pub fn load_settings(app: &AppHandle) -> AmbientSettings {
    settings_path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_settings(app: &AppHandle, s: &AmbientSettings) {
    if let Some(p) = settings_path(app) {
        if let Ok(json) = serde_json::to_string_pretty(s) {
            let _ = std::fs::write(p, json);
        }
    }
}

// --- burn-rate history (file-backed, shared with the popover) ---------------

use crate::burn::{self, Sample};

/// Keep a little more than the burn lookback so a baseline is always on hand.
const HISTORY_RETAIN_SECS: i64 = burn::LOOKBACK_SECS + 30 * 60;

fn history_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    let dir = app.path().app_config_dir().ok()?;
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("history.json"))
}

fn load_history(app: &AppHandle) -> HashMap<String, Vec<Sample>> {
    history_path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_history(app: &AppHandle, h: &HashMap<String, Vec<Sample>>) {
    let Some(p) = history_path(app) else { return };
    let Ok(json) = serde_json::to_string(h) else { return };
    // Write-then-rename so a popover read never sees a half-written file.
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

fn now_unix() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Seconds from now until an RFC3339 instant (negative if past).
fn secs_until(iso: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(iso)
        .ok()
        .map(|dt| dt.timestamp() - now_unix())
}

/// Append the current utilisation of each resettable window to the rolling
/// history, dropping a window's samples when it resets (a drop in used) and
/// pruning anything past the retention horizon. Called once per background poll.
fn record_history(app: &AppHandle, report: &UsageReport) {
    let mut hist = load_history(app);
    let now = now_unix();
    for w in &report.windows {
        // Only windows with a real reset clock are projectable.
        if w.trailing.as_deref() == Some("Off") || w.resets_at.is_none() {
            continue;
        }
        let v = hist.entry(w.title.clone()).or_default();
        if let Some(last) = v.last() {
            if w.utilization + 1e-9 < last.used {
                v.clear(); // window reset since last sample
            }
        }
        v.push(Sample {
            ts: now,
            used: w.utilization,
        });
        v.retain(|s| now - s.ts <= HISTORY_RETAIN_SECS);
    }
    save_history(app, &hist);
}

/// Fill in each window's `eta_secs` from the recorded history — but only when the
/// projected limit-hit falls *before* the window resets (an actionable "you'll
/// run out" warning). Used by the `fetch_usage` command so the popover can show
/// it. No-op for windows with no reset clock or not enough burn.
pub fn annotate(app: &AppHandle, report: &mut UsageReport) {
    let hist = load_history(app);
    let now = now_unix();
    for w in &mut report.windows {
        if w.trailing.as_deref() == Some("Off") {
            continue;
        }
        let samples = hist.get(&w.title).map(|v| v.as_slice()).unwrap_or(&[]);
        let Some(eta) = burn::eta_to_empty(samples, now, w.utilization) else {
            continue;
        };
        // Surface only if you'd hit the limit before the window resets.
        let before_reset = match w.resets_at.as_deref().and_then(secs_until) {
            Some(reset_in) => eta < reset_in,
            None => true,
        };
        if before_reset {
            w.eta_secs = Some(eta);
        }
    }
}

// --- the background poller ---------------------------------------------------

/// Spawn the ambient-HP loop. Cheap no-op cost when signed out (one failed fetch
/// per interval), so it can run unconditionally for the app's lifetime.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(STARTUP_DELAY_SECS)).await;
        // Dev hook: `HPBAR_TRAY_DEMO=<0..1>` paints the heart at a fixed level and
        // skips polling, so the tray rendering can be eyeballed without live creds.
        // `HPBAR_TRAY_THEME=minecraft|classic|arknights` overrides the theme.
        if let Some(r) = std::env::var("HPBAR_TRAY_DEMO")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
        {
            let theme = std::env::var("HPBAR_TRAY_THEME")
                .map(|t| heart_icon::TrayTheme::from_id(&t))
                .unwrap_or_else(|_| current_theme(&app));
            paint(&app, r, &format!("HPBar demo · {}%", pct(r)), theme);
            return;
        }
        // Per-window last-notified severity, so each depletion alerts once (and
        // re-arms only after the window resets back to healthy). In-memory: a
        // duplicate alert after an app restart is the acceptable worst case.
        let mut last: HashMap<String, u8> = HashMap::new();
        loop {
            if let Ok(report) = fetch(&app).await {
                record_history(&app, &report); // feed the burn-rate projection
                apply(&app, &report);
                let alerts_on = load_settings(&app).alerts_enabled;
                notify_crossings(&app, &report, &mut last, alerts_on);
            }
            tokio::time::sleep(Duration::from_secs(POLL_SECS)).await;
        }
    });
}

async fn fetch(app: &AppHandle) -> Result<UsageReport, ()> {
    let cache = app.state::<CredentialCache>();
    usage::fetch(cache.inner()).await.map_err(|_| ())
}

/// Redraw the tray icon + tooltip from the report, and stash the HP + tooltip so
/// a later theme switch can repaint instantly.
fn apply(app: &AppHandle, report: &UsageReport) {
    if let Some(remaining) = min_remaining(report) {
        let tip = tooltip(report);
        if let Some(st) = app.try_state::<TrayState>() {
            *st.0.lock().unwrap() = Some((remaining, tip.clone()));
        }
        paint(app, remaining, &tip, current_theme(app));
    }
}

/// The tray theme from persisted settings.
fn current_theme(app: &AppHandle) -> heart_icon::TrayTheme {
    heart_icon::TrayTheme::from_id(&load_settings(app).theme)
}

/// Paint the tray heart at `remaining` (0..1) with `tooltip` hover text.
///
/// Updates run on the main thread — tray back-ends (macOS `NSStatusItem`, the
/// Windows message pump, GTK on Linux) are all main-thread-only — so we marshal
/// once and batch the updates there.
fn paint(app: &AppHandle, remaining: f64, tooltip: &str, theme: heart_icon::TrayTheme) {
    let (rgba, w, h) = heart_icon::render_rgba(remaining, SCALE, theme);
    // Annunciator: when low, show the exact "NN%" as a tray *title* — menu-bar
    // text the OS keeps legible on any wallpaper, which the icon hue can't (macOS
    // menu-bar vibrancy can wash the colour out entirely). macOS-only on purpose:
    // `set_title` is unsupported on Windows and inconsistent / panel-cluttering on
    // Linux, and both of those render the icon's colour faithfully anyway.
    #[cfg(target_os = "macos")]
    let title = (remaining <= LOW).then(|| format!("{}%", pct(remaining)));
    let app = app.clone();
    let tooltip = tooltip.to_string();
    let _ = app.clone().run_on_main_thread(move || {
        if let Some(tray) = app.tray_by_id(TRAY_ID) {
            // Set the icon (and, on macOS, template=false) atomically: doing
            // set_icon then set_icon_as_template separately renders the heart
            // twice → a visible flicker. On Linux/Windows this just sets the icon.
            let _ = tray.set_icon_with_as_template(Some(Image::new_owned(rgba, w, h)), false);
            let _ = tray.set_tooltip(Some(&tooltip));
            #[cfg(target_os = "macos")]
            let _ = tray.set_title(title.as_deref());
        }
    });
}

/// Fire a notification for any window that just crossed *into* a worse severity.
/// `last` carries each window's previously-notified severity across polls.
fn notify_crossings(
    app: &AppHandle,
    report: &UsageReport,
    last: &mut HashMap<String, u8>,
    alerts_on: bool,
) {
    for w in &report.windows {
        if w.trailing.as_deref() == Some("Off") {
            continue;
        }
        let sev = severity(w.remaining);
        let prev = last.get(&w.title).copied().unwrap_or(0);
        if sev > prev && alerts_on {
            let (title, body) = alert_text(&w.title, w.remaining, sev);
            let _ = app.notification().builder().title(title).body(body).show();
        }
        // Track the new severity either way, so toggling alerts off doesn't queue
        // up a backlog that fires the instant they're re-enabled.
        last.insert(w.title.clone(), sev);
    }
}

fn alert_text(window: &str, remaining: f64, sev: u8) -> (String, String) {
    let left = pct(remaining);
    if sev >= 2 {
        (
            format!("Claude quota critical — {window}"),
            format!("Only {left}% left on your {window} window. You may be rate-limited soon."),
        )
    } else {
        (
            format!("Claude quota low — {window}"),
            format!("{left}% left on your {window} window."),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{UsageReport, UsageWindow};

    fn win(title: &str, remaining: f64, off: bool) -> UsageWindow {
        UsageWindow {
            utilization: 1.0 - remaining,
            remaining,
            resets_at: None,
            title: title.into(),
            trailing: off.then(|| "Off".into()),
            eta_secs: None,
        }
    }
    fn report(windows: Vec<UsageWindow>) -> UsageReport {
        UsageReport {
            windows,
            source_label: "test".into(),
        }
    }

    #[test]
    fn min_remaining_picks_worst_active_window() {
        let r = report(vec![win("5-Hour", 0.8, false), win("Weekly", 0.3, false)]);
        assert_eq!(min_remaining(&r), Some(0.3));
    }

    #[test]
    fn min_remaining_skips_off_windows() {
        // The "Off" extra-usage window (remaining 0.0) must not pin HP to zero.
        let r = report(vec![win("5-Hour", 0.6, false), win("Extra usage", 0.0, true)]);
        assert_eq!(min_remaining(&r), Some(0.6));
    }

    #[test]
    fn severity_buckets() {
        assert_eq!(severity(0.9), 0);
        assert_eq!(severity(0.20), 1);
        assert_eq!(severity(0.06), 1);
        assert_eq!(severity(0.05), 2);
        assert_eq!(severity(0.0), 2);
    }

    #[test]
    fn crossings_fire_once_then_rearm_on_reset() {
        // Drive the severity machine directly (no AppHandle / no real notifs).
        let mut last: HashMap<String, u8> = HashMap::new();
        let step = |last: &mut HashMap<String, u8>, remaining: f64| -> bool {
            let sev = severity(remaining);
            let prev = last.get("5-Hour").copied().unwrap_or(0);
            let fired = sev > prev;
            last.insert("5-Hour".into(), sev);
            fired
        };
        assert!(!step(&mut last, 0.5)); // healthy → no alert
        assert!(step(&mut last, 0.15)); // crossed into low → alert
        assert!(!step(&mut last, 0.12)); // still low → no repeat
        assert!(step(&mut last, 0.03)); // crossed into critical → alert
        assert!(!step(&mut last, 0.01)); // still critical → no repeat
        assert!(!step(&mut last, 0.9)); // window reset → re-arm, no alert
        assert!(step(&mut last, 0.10)); // next depletion → alert again
    }

    #[test]
    fn tooltip_formats_windows() {
        let r = report(vec![win("5-Hour", 0.47, false), win("Extra usage", 0.0, true)]);
        assert_eq!(tooltip(&r), "HPBar · 5-Hour 47% · Extra usage off");
    }
}
