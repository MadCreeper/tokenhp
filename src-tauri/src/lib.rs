//! HPBar (Tauri) — Claude subscription usage in the system tray / menu bar.
//!
//! Port of the macOS-only SwiftUI app. The Rust side owns the tray icon, a
//! borderless popover window, and the credential + usage-fetch logic; the web
//! frontend renders the health bars.

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

/// Local per-model token breakdown over the last `window_secs`. Scans session
/// transcripts on a blocking thread so the UI stays responsive.
#[tauri::command]
async fn fetch_local(window_secs: i64) -> Result<localstats::LocalReport, String> {
    tokio::task::spawn_blocking(move || localstats::fetch(window_secs))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(CredentialCache::new())
        .invoke_handler(tauri::generate_handler![fetch_usage, fetch_local])
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
    let menu = Menu::with_items(
        app,
        &[&autostart, &PredefinedMenuItem::separator(app)?, &quit],
    )?;
    // Captured so the toggle handler can reflect the new state in the checkmark.
    let autostart_item = autostart.clone();

    // tray.png is a monochrome heart; `icon_as_template` lets macOS recolor it
    // to match the menu bar (light/dark) automatically.
    let icon = Image::from_bytes(include_bytes!("../icons/tray.png"))
        .expect("bundled tray.png must be a valid PNG");

    TrayIconBuilder::with_id("main")
        .icon(icon)
        .icon_as_template(true)
        .tooltip("HPBar")
        .menu(&menu)
        .show_menu_on_left_click(false) // left click = popover, right click = menu
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "quit" => app.exit(0),
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

/// Place the window just below the tray icon, right edge aligned to the icon
/// (like the AppKit version), clamped to the icon's monitor so it never spills
/// off-screen — menu-bar icons sit at the right edge, so a naive centered
/// placement would hang halfway off the display.
fn position_under_tray(win: &WebviewWindow, rect: Rect) {
    use tauri::{PhysicalPosition, Position, Size};

    let scale = win.scale_factor().unwrap_or(1.0);

    // The tray rect is reported in the menu bar's coordinate space. Resolve the
    // icon's top-left + size to physical pixels.
    let (icon_x, icon_y) = match rect.position {
        Position::Physical(p) => (p.x as f64, p.y as f64),
        Position::Logical(p) => (p.x * scale, p.y * scale),
    };
    let (icon_w, icon_h) = match rect.size {
        Size::Physical(s) => (s.width as f64, s.height as f64),
        Size::Logical(s) => (s.width * scale, s.height * scale),
    };

    let win_w = win.outer_size().map(|s| s.width as f64).unwrap_or(360.0);

    // A few px below the icon so the revealed menu bar never covers it.
    const GAP: f64 = 6.0;
    let mut x = icon_x + icon_w - win_w; // right edge under the icon's right edge
    let y = icon_y + icon_h + GAP;

    // Clamp x to the icon's monitor so the panel stays fully on-screen.
    if let Ok(Some(monitor)) = win.current_monitor() {
        let mp = monitor.position();
        let ms = monitor.size();
        let min_x = mp.x as f64 + 8.0;
        let max_x = (mp.x as f64 + ms.width as f64) - win_w - 8.0;
        x = x.clamp(min_x, max_x.max(min_x));
    } else {
        x = x.max(0.0);
    }

    let _ = win.set_position(PhysicalPosition::new(x, y.max(0.0)));
}
