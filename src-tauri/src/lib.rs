//! HPBar (Tauri) — Claude subscription usage in the system tray / menu bar.
//!
//! Port of the macOS-only SwiftUI app. The Rust side owns the tray icon, a
//! borderless popover window, and the credential + usage-fetch logic; the web
//! frontend renders the health bars.

pub mod account;
pub mod codexstats;
pub mod credentials;
pub mod localstats;
pub mod pricing;
pub mod usage;

use credentials::CredentialCache;
use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, Rect, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_autostart::ManagerExt;

/// Frontend calls this to get the current live quota. Returns the report or a
/// human-readable error string (the frontend renders either).
#[tauri::command]
async fn fetch_usage(
    cache: tauri::State<'_, CredentialCache>,
) -> Result<usage::UsageReport, String> {
    usage::fetch(cache.inner())
        .await
        .map_err(|e| e.to_string())
}

/// Login identity for the footer (email + plan). Best-effort: returns whatever
/// can be read from local Claude Code state, with empty fields otherwise.
#[tauri::command]
fn fetch_account(cache: tauri::State<'_, CredentialCache>) -> account::AccountInfo {
    account::fetch(cache.inner())
}

/// Local per-model token breakdown over the last `window_secs`. Scans session
/// transcripts on a blocking thread so the UI stays responsive.
#[tauri::command]
async fn fetch_local(window_secs: i64) -> Result<localstats::LocalReport, String> {
    tokio::task::spawn_blocking(move || localstats::fetch(window_secs))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Codex per-model token breakdown over the last `window_secs`, from
/// `~/.codex/sessions`. Same shape as `fetch_local` so the UI reuses it.
#[tauri::command]
async fn fetch_codex_local(window_secs: i64) -> Result<localstats::LocalReport, String> {
    tokio::task::spawn_blocking(move || codexstats::fetch_local(window_secs))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Codex's latest local rate-limit snapshot, shaped like the live-quota bars.
#[tauri::command]
async fn fetch_codex_quota() -> Result<usage::UsageReport, String> {
    tokio::task::spawn_blocking(codexstats::fetch_quota)
        .await
        .map_err(|e| e.to_string())?
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(CredentialCache::new())
        .invoke_handler(tauri::generate_handler![
            fetch_usage,
            fetch_local,
            fetch_codex_local,
            fetch_codex_quota,
            fetch_account
        ])
        .setup(|app| {
            // macOS: run as a menu-bar accessory — no Dock icon, no app menu.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            build_popover(app.handle())?;
            build_tray(app.handle())?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running HPBar");
}

/// The borderless panel anchored under the tray icon. Created hidden; shown on
/// tray click. We `hide()` rather than close it so its webview (and JS poll
/// loop) stays alive between opens.
fn build_popover(app: &tauri::AppHandle) -> tauri::Result<()> {
    let popover = WebviewWindowBuilder::new(app, "popover", WebviewUrl::App("index.html".into()))
        .title("HPBar")
        .inner_size(360.0, 300.0)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .build()?;

    // Dismiss like a real menu-bar popover: hide as soon as focus leaves.
    let win = popover.clone();
    popover.on_window_event(move |event| {
        if let WindowEvent::Focused(false) = event {
            // Swallow the spurious focus-out some Linux compositors fire while
            // the window is still appearing — without this the popover would
            // flash open and immediately hide itself. macOS/Windows are unchanged.
            #[cfg(target_os = "linux")]
            if shown_recently() {
                return;
            }
            let _ = win.hide();
        }
    });

    Ok(())
}

fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let launch_enabled = app.autolaunch().is_enabled().unwrap_or(false);
    let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        "Open at Login",
        true,
        launch_enabled,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit HPBar", true, None::<&str>)?;
    // Linux trays (libappindicator/StatusNotifierItem) don't deliver left-click
    // events, so the only reliable way to open the popover there is a menu item.
    // Harmless on macOS/Windows, where left-click still works too.
    let show = MenuItem::with_id(app, "show", "Show HPBar", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &show,
            &PredefinedMenuItem::separator(app)?,
            &autostart,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    // Captured so the toggle handler can reflect the new state in the checkmark.
    let autostart_item = autostart.clone();

    // tray.png is a monochrome heart; `icon_as_template` lets macOS recolor it
    // to match the menu bar (light/dark) automatically.
    let icon = Image::from_bytes(include_bytes!("../icons/tray.png"))
        .expect("bundled tray.png must be a valid PNG");

    // On Linux the tray never reports clicks, so the menu must be reachable by
    // *left* click — that's where users will find "Show HPBar". On macOS/Windows
    // clicks work, so we reserve left-click for the popover and put the menu on
    // right-click.
    #[cfg(target_os = "linux")]
    let show_menu_on_left_click = true;
    #[cfg(not(target_os = "linux"))]
    let show_menu_on_left_click = false;

    TrayIconBuilder::with_id("main")
        .icon(icon)
        .icon_as_template(true)
        .tooltip("HPBar")
        .menu(&menu)
        .show_menu_on_left_click(show_menu_on_left_click)
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "quit" => app.exit(0),
            "show" => {
                if let Some(win) = app.get_webview_window("popover") {
                    toggle_popover_no_anchor(&win);
                }
            }
            "autostart" => {
                let mgr = app.autolaunch();
                let enabled = mgr.is_enabled().unwrap_or(false);
                let _ = if enabled { mgr.disable() } else { mgr.enable() };
                let _ = autostart_item.set_checked(!enabled);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                if let Some(win) = tray.app_handle().get_webview_window("popover") {
                    toggle_popover(&win, rect);
                }
            }
        })
        .build(app)?;

    Ok(())
}

/// Show the popover anchored beneath the clicked tray icon, or hide it if it's
/// already visible.
fn toggle_popover(win: &WebviewWindow, rect: Rect) {
    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
        return;
    }

    position_under_tray(win, rect);
    let _ = win.show();
    let _ = win.set_focus();
    // Tell the frontend to re-fetch now that we're visible.
    let _ = win.emit("refresh", ());
}

/// Toggle the popover when we have no tray rectangle to anchor to — i.e. it was
/// opened from the tray menu rather than a click. This is the only path that
/// works on Linux, where the tray backend never emits left-click events.
fn toggle_popover_no_anchor(win: &WebviewWindow) {
    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
        return;
    }

    position_top_right(win);
    mark_shown();
    let _ = win.show();
    let _ = win.set_focus();
    let _ = win.emit("refresh", ());
}

/// Anchor the popover to the top-right corner of the primary monitor — the
/// usual home of the system tray on Linux desktops (GNOME/KDE) — when we don't
/// have the icon's own rectangle to position against.
fn position_top_right(win: &WebviewWindow) {
    use tauri::PhysicalPosition;

    let win_w = win.outer_size().map(|s| s.width as f64).unwrap_or(360.0);

    let monitor = win
        .primary_monitor()
        .ok()
        .flatten()
        .or_else(|| win.current_monitor().ok().flatten());

    if let Some(monitor) = monitor {
        let mp = monitor.position();
        let ms = monitor.size();
        const MARGIN: f64 = 8.0;
        let x = mp.x as f64 + ms.width as f64 - win_w - MARGIN;
        let y = mp.y as f64 + MARGIN;
        let _ = win.set_position(PhysicalPosition::new(x, y));
    }
}

// --- Post-show grace period (best-of-both: layered on top of the verified
// Linux tray fix). Guards the blur-to-hide handler against the spurious
// focus-out some Linux compositors emit while the popover is still appearing.

/// Monotonic clock anchored at first use; basis for the post-show grace period.
fn app_clock() -> &'static std::time::Instant {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now)
}

static LAST_SHOWN_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Record the moment of the most recent show, so [`shown_recently`] can ignore
/// the focus-out that immediately follows on some Linux compositors.
fn mark_shown() {
    LAST_SHOWN_MS.store(
        app_clock().elapsed().as_millis() as u64,
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// True for a brief window after a show. Only consulted on Linux.
#[allow(dead_code)]
fn shown_recently() -> bool {
    const GRACE_MS: u64 = 250;
    let now = app_clock().elapsed().as_millis() as u64;
    now.saturating_sub(LAST_SHOWN_MS.load(std::sync::atomic::Ordering::Relaxed)) < GRACE_MS
}

/// Place the popover next to the tray icon, right-edge aligned, on the icon's
/// own monitor. Opens *below* the icon when it's near the top of its screen
/// (macOS menu bar) and *above* when near the bottom (Windows taskbar tray) —
/// so it works regardless of where the tray lives. All math is in global
/// physical pixels, which span the whole multi-display layout (offsets can be
/// negative), so we never floor coordinates to 0.
fn position_under_tray(win: &WebviewWindow, rect: Rect) {
    use tauri::{PhysicalPosition, Position, Size};

    let scale = win.scale_factor().unwrap_or(1.0);

    let (icon_x, icon_y) = match rect.position {
        Position::Physical(p) => (p.x as f64, p.y as f64),
        Position::Logical(p) => (p.x * scale, p.y * scale),
    };
    let (icon_w, icon_h) = match rect.size {
        Size::Physical(s) => (s.width as f64, s.height as f64),
        Size::Logical(s) => (s.width * scale, s.height * scale),
    };

    let (win_w, win_h) = win
        .outer_size()
        .map(|s| (s.width as f64, s.height as f64))
        .unwrap_or((360.0, 300.0));

    // Find the monitor the icon is ON — not the window's current monitor, which
    // would yank the panel back onto the main display in a multi-screen setup.
    let icon_monitor = win
        .available_monitors()
        .ok()
        .and_then(|monitors| {
            monitors.into_iter().find(|m| {
                let mp = m.position();
                let ms = m.size();
                let (mx, my) = (mp.x as f64, mp.y as f64);
                icon_x >= mx
                    && icon_x < mx + ms.width as f64
                    && icon_y >= my
                    && icon_y < my + ms.height as f64
            })
        })
        .or_else(|| win.current_monitor().ok().flatten());

    const GAP: f64 = 6.0;
    let mut x = icon_x + icon_w - win_w; // right edge under the icon's right edge

    // Below the icon by default (top menu bar); above it if the icon sits in the
    // bottom half of its monitor (a bottom taskbar tray, e.g. Windows).
    let icon_in_bottom_half = icon_monitor.as_ref().is_some_and(|m| {
        let mid = m.position().y as f64 + m.size().height as f64 / 2.0;
        icon_y > mid
    });
    let mut y = if icon_in_bottom_half {
        icon_y - win_h - GAP
    } else {
        icon_y + icon_h + GAP
    };

    // Clamp onto the icon's monitor so the panel stays fully on-screen.
    if let Some(monitor) = &icon_monitor {
        let mp = monitor.position();
        let ms = monitor.size();
        let (mx, my) = (mp.x as f64, mp.y as f64);
        x = x.clamp(mx + 8.0, (mx + ms.width as f64 - win_w - 8.0).max(mx + 8.0));
        y = y.clamp(my + 8.0, (my + ms.height as f64 - win_h - 8.0).max(my + 8.0));
    }

    let _ = win.set_position(PhysicalPosition::new(x, y));
}
