//! HPBar (Tauri) — Claude subscription usage in the system tray / menu bar.
//!
//! Port of the macOS-only SwiftUI app. The Rust side owns the tray icon, a
//! borderless popover window, and the credential + usage-fetch logic; the web
//! frontend renders the health bars.

pub mod account;
pub mod ambient;
pub mod burn;
pub mod codexstats;
pub mod credentials;
pub mod heart_icon;
pub mod localstats;
pub mod openclawstats;
pub mod pricing;
pub mod share;
pub mod team;
pub mod tools;
pub mod update;
pub mod usage;

use credentials::CredentialCache;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "macos")]
use std::sync::atomic::AtomicUsize;
use std::time::Duration;
use tauri::{
    image::Image,
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, Rect, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
// The native tray menu is Linux/Windows-only now (see `build_tray`); macOS hosts
// those controls in the popover instead, so these menu types are unused there.
#[cfg(not(target_os = "macos"))]
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri_plugin_autostart::ManagerExt;

/// While pinned the popover ignores focus loss and floats like a desktop
/// widget (the frontend renders the glassy "unfocused" look). Toggled by the
/// header pin button; not persisted here — the frontend re-syncs it on launch
/// from its own stored preference.
static PINNED: AtomicBool = AtomicBool::new(false);

#[tauri::command]
fn set_pinned(pinned: bool) {
    PINNED.store(pinned, Ordering::Relaxed);
}

/// The popover's NSGlassEffectView, kept alive by its superview. Stored raw so
/// the Settings toggle can hide/show it. 0 = not created (pre-26 fallback, or
/// non-macOS).
#[cfg(target_os = "macos")]
static GLASS_VIEW: AtomicUsize = AtomicUsize::new(0);

/// Frontend "Liquid Glass" toggle: show/hide the native glass backdrop. When
/// hidden the classic panel renders opaque (CSS drops the `glass` class), so the
/// popover falls back to a plain rounded surface. macOS-only; a no-op elsewhere
/// (and when the glass view was never created).
#[tauri::command]
fn set_glass_enabled(enabled: bool) {
    let _ = enabled; // used on macOS only
    #[cfg(target_os = "macos")]
    {
        let ptr = GLASS_VIEW.load(Ordering::Relaxed);
        if ptr == 0 {
            return;
        }
        // Window events / commands run on the main thread → AppKit is safe here.
        use objc2::msg_send;
        use objc2::runtime::{AnyObject, Bool};
        unsafe {
            let glass = ptr as *mut AnyObject;
            let _: () = msg_send![glass, setHidden: Bool::new(!enabled)];
        }
    }
}

/// Frontend calls this to get the current live quota. Returns the report or a
/// human-readable error string (the frontend renders either).
#[tauri::command]
async fn fetch_usage(
    app: tauri::AppHandle,
    cache: tauri::State<'_, CredentialCache>,
) -> Result<usage::UsageReport, String> {
    let mut report = usage::fetch(cache.inner()).await.map_err(|e| e.to_string())?;
    // Add the "you'll run out before reset" projection from recorded history.
    ambient::annotate(&app, &mut report);
    // Add the this-machine vs other-devices split from the recorded series.
    share::annotate(&app, "claude", &mut report);
    Ok(report)
}

/// Login identity for the footer (email + plan). Best-effort: returns whatever
/// can be read from local Claude Code state, with empty fields otherwise.
#[tauri::command]
fn fetch_account(cache: tauri::State<'_, CredentialCache>) -> account::AccountInfo {
    account::fetch(cache.inner())
}

/// Codex (ChatGPT) login identity for the footer, from `~/.codex/auth.json`.
#[tauri::command]
fn fetch_codex_account() -> account::AccountInfo {
    account::fetch_codex()
}

/// The API axis: per-tool + pooled token usage over the last `window_secs`,
/// aggregating every local tool (Claude Code, Codex, …). Scans logs on a
/// blocking thread so the UI stays responsive.
#[tauri::command]
async fn fetch_local(window_secs: i64) -> Result<tools::LocalReport, String> {
    tokio::task::spawn_blocking(move || tools::fetch_local(window_secs))
        .await
        .map_err(|e| e.to_string())?
}

/// Codex's latest local rate-limit snapshot, shaped like the live-quota bars,
/// with the device-share split recorded + annotated.
#[tauri::command]
async fn fetch_codex_quota(app: tauri::AppHandle) -> Result<usage::UsageReport, String> {
    let mut report = tokio::task::spawn_blocking(codexstats::fetch_quota)
        .await
        .map_err(|e| e.to_string())??;
    // Record this machine's Codex share sample (scans logs → blocking thread),
    // then annotate the report with the this-machine vs others split.
    let app2 = app.clone();
    let report2 = report.clone();
    let _ = tokio::task::spawn_blocking(move || share::record(&app2, "codex", &report2)).await;
    share::annotate(&app, "codex", &mut report);
    Ok(report)
}

/// Mirror the popover's theme onto the tray heart, repainting it now. Called by
/// the frontend on startup and whenever the user cycles the theme.
#[tauri::command]
fn set_tray_theme(app: tauri::AppHandle, theme: String) {
    ambient::set_theme_and_repaint(&app, theme);
}

// --- App controls (relocated from the tray menu) -----------------------------
// On macOS 26 (Tahoe) a status item with a bound menu shows that menu on *left*
// click, stealing the click that should open the popover — and there's no API to
// keep a menu object without binding it natively (verified through tray-icon
// 0.24.1). So macOS drops the native tray menu entirely and the popover's
// Settings hosts these controls instead. See `build_tray`.

/// Current state of the relocated tray-menu toggles, for the Settings UI.
#[derive(serde::Serialize)]
struct AppControls {
    autostart: bool,
    alerts: bool,
    calibrate: bool,
}

#[tauri::command]
fn get_app_controls(app: tauri::AppHandle) -> AppControls {
    let s = ambient::load_settings(&app);
    AppControls {
        autostart: app.autolaunch().is_enabled().unwrap_or(false),
        alerts: s.alerts_enabled,
        calibrate: s.only_active_device,
    }
}

#[tauri::command]
fn set_autostart(app: tauri::AppHandle, enabled: bool) {
    let mgr = app.autolaunch();
    let _ = if enabled { mgr.enable() } else { mgr.disable() };
}

#[tauri::command]
fn set_alerts_enabled(app: tauri::AppHandle, enabled: bool) {
    let mut s = ambient::load_settings(&app);
    s.alerts_enabled = enabled;
    ambient::save_settings(&app, &s);
}

#[tauri::command]
fn set_calibrate(app: tauri::AppHandle, enabled: bool) {
    let mut s = ambient::load_settings(&app);
    s.only_active_device = enabled;
    ambient::save_settings(&app, &s);
}

/// Quit the app. On macOS this is the only quit affordance now that the tray has
/// no native menu (the app is a Dock-less accessory), so the popover owns it.
#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

// --- Team sharing (opt-in) ---------------------------------------------------

/// Current team-sharing config (defaults, with `enabled=false`, when unset). The
/// frontend uses `enabled` to decide whether to show the Team tab at all.
#[tauri::command]
fn get_team_config() -> team::TeamConfig {
    team::TeamConfig::load()
}

/// Persist team-sharing config, filling in derived fields (member id, display
/// name) from the local account first.
#[tauri::command]
fn set_team_config(mut config: team::TeamConfig) -> Result<(), String> {
    config.normalize(account::read_email().as_deref());
    config.save()
}

/// Verify connectivity/auth (open the SSH tunnel, connect to Postgres, migrate),
/// write this member's row, and return the current roster — the "handshake"
/// shown in settings.
#[tauri::command]
async fn test_team_connection(
    mut config: team::TeamConfig,
) -> Result<team::TeamHandshake, String> {
    let email = account::read_email();
    config.normalize(email.as_deref());
    team::test_connection(&config, email.as_deref()).await
}

/// Push this member's latest snapshot now (also done periodically in the
/// background). No-op when sharing is disabled.
#[tauri::command]
async fn upload_team_snapshot() -> Result<(), String> {
    let mut cfg = team::TeamConfig::load();
    if !cfg.enabled {
        return Ok(());
    }
    let email = account::read_email();
    cfg.normalize(email.as_deref());
    team::upload(&cfg, email.as_deref()).await
}

/// The team leaderboard for `range` ("day" | "week" | "month").
#[tauri::command]
async fn fetch_team(range: String) -> Result<team::TeamReport, String> {
    team::fetch_team(&range).await
}

/// Background loop: every `interval_secs`, push this member's snapshot if team
/// sharing is enabled. Cheap no-op (a config read + sleep) when disabled, so it
/// can run unconditionally for the app's lifetime.
fn spawn_team_uploader() {
    tauri::async_runtime::spawn(async move {
        // Small initial delay so startup isn't competing with the first render.
        tokio::time::sleep(Duration::from_secs(20)).await;
        loop {
            let mut cfg = team::TeamConfig::load();
            let interval = cfg.interval_secs.max(600);
            if cfg.enabled && !cfg.ssh_host.trim().is_empty() {
                let email = account::read_email();
                cfg.normalize(email.as_deref());
                let _ = team::upload(&cfg, email.as_deref()).await;
            }
            tokio::time::sleep(Duration::from_secs(interval)).await;
        }
    });
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_notification::init())
        .manage(CredentialCache::new())
        .manage(ambient::TrayState::default())
        .invoke_handler(tauri::generate_handler![
            fetch_usage,
            fetch_local,
            fetch_codex_quota,
            fetch_account,
            fetch_codex_account,
            set_tray_theme,
            set_pinned,
            set_glass_enabled,
            get_app_controls,
            set_autostart,
            set_alerts_enabled,
            set_calibrate,
            quit_app,
            get_team_config,
            set_team_config,
            test_team_connection,
            upload_team_snapshot,
            fetch_team,
            update::app_version,
            update::check_update,
            update::download_and_install_update,
            update::open_external
        ])
        .setup(|app| {
            // macOS: run as a menu-bar accessory — no Dock icon, no app menu.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            build_popover(app.handle())?;
            build_tray(app.handle())?;
            // Debug aid: HPBAR_DEBUG_SHOW=1 shows the popover pinned at launch
            // (no tray interaction needed) and, with HPBAR_DEBUG_CYCLE=1,
            // deactivates the app after 8s — lets the focused/unfocused glass
            // states be screenshotted non-interactively.
            #[cfg(target_os = "macos")]
            if std::env::var_os("HPBAR_DEBUG_SHOW").is_some() {
                PINNED.store(true, Ordering::Relaxed);
                if let Some(win) = app.get_webview_window("popover") {
                    position_top_right(&win);
                    let _ = win.show();
                    let _ = win.set_focus();
                    // set_focus alone can't activate an Accessory app launched
                    // from a background shell — force it.
                    unsafe {
                        use objc2::msg_send;
                        use objc2::runtime::{AnyObject, Bool};
                        let napp: *mut AnyObject =
                            msg_send![objc2::class!(NSApplication), sharedApplication];
                        let _: () = msg_send![napp, activateIgnoringOtherApps: Bool::new(true)];
                    }
                }
                if std::env::var_os("HPBAR_DEBUG_CYCLE").is_some() {
                    let handle = app.handle().clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_secs(8));
                        let _ = handle.run_on_main_thread(|| {
                            use objc2::msg_send;
                            use objc2::runtime::AnyObject;
                            unsafe {
                                let napp: *mut AnyObject =
                                    msg_send![objc2::class!(NSApplication), sharedApplication];
                                let _: () = msg_send![napp, deactivate];
                            }
                        });
                    });
                }
            }
            spawn_team_uploader();
            // Ambient HP: keep the menu-bar heart + tooltip live, and alert on
            // low/critical quota. Runs for the app's lifetime; no-op when signed out.
            ambient::spawn(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running HPBar");
}

/// The borderless panel anchored under the tray icon. Created hidden; shown on
/// tray click. We `hide()` rather than close it so its webview (and JS poll
/// loop) stays alive between opens.
fn build_popover(app: &tauri::AppHandle) -> tauri::Result<()> {
    let builder = WebviewWindowBuilder::new(app, "popover", WebviewUrl::App("index.html".into()))
        .title("HPBar")
        .inner_size(360.0, 300.0)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false);
    // Liquid glass (macOS): the window itself must be transparent so the native
    // backdrop we attach below shows through. The frontend then renders a lightly
    // tinted panel on top (classic theme). Other platforms keep the solid look.
    #[cfg(target_os = "macos")]
    let builder = builder.transparent(true);
    let popover = builder.build()?;

    #[cfg(target_os = "macos")]
    {
        // Backdrop, best first: real Liquid Glass (NSGlassEffectView, macOS 26+,
        // "clear" style — closest to system UI per the glass_check example), else
        // NSVisualEffectView popover vibrancy on older macOS.
        // 18px corner radius to match the classic panel's widget-like rounding.
        if !apply_liquid_glass(&popover, 18.0) {
            use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};
            if let Err(e) = apply_vibrancy(
                &popover,
                NSVisualEffectMaterial::Popover,
                Some(NSVisualEffectState::Active),
                Some(18.0),
            ) {
                eprintln!("[hpbar] vibrancy fallback failed: {e:?}");
            }
        }

        // wry disables `drawsBackground` for a transparent webview but leaves
        // WKWebView's `underPageBackgroundColor` at its opaque macOS-12+ default,
        // which paints white behind our transparent page — hiding the backdrop.
        // Clear it (and force the view non-opaque) so the glass shows.
        let r = popover.with_webview(|webview| {
            use objc2::runtime::{AnyObject, Bool};
            use objc2::{class, msg_send, sel};
            unsafe {
                let wk = webview.inner() as *mut AnyObject;
                if wk.is_null() {
                    eprintln!("[hpbar] webview inner() was null");
                    return;
                }
                let _: () = msg_send![wk, setOpaque: Bool::new(false)];
                let responds: Bool = msg_send![wk, respondsToSelector: sel!(setUnderPageBackgroundColor:)];
                if responds.as_bool() {
                    let clear: *mut AnyObject = msg_send![class!(NSColor), clearColor];
                    let _: () = msg_send![wk, setUnderPageBackgroundColor: clear];
                }
            }
        });
        if let Err(e) = r {
            eprintln!("[hpbar] with_webview failed: {e:?}");
        }
    }

    // Dismiss like a real menu-bar popover: hide as soon as focus leaves.
    let win = popover.clone();
    popover.on_window_event(move |event| {
        if let WindowEvent::Focused(focused) = event {
            // Mirror focus into the page as a root class (widget styling: opaque
            // in focus, clear glass out of focus). Direct eval — the JS-side
            // onFocusChanged listener proved unreliable here (silently never
            // fired), while this Rust event is what hide-on-blur already trusts.
            let _ = win.eval(format!(
                "document.documentElement.classList.toggle('blurred', {})",
                !*focused
            ));
            if *focused {
                return;
            }
            // Pinned: float like a desktop widget instead of dismissing.
            if PINNED.load(Ordering::Relaxed) {
                return;
            }
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

/// Insert an NSGlassEffectView — real Liquid Glass, macOS 26+ — behind the
/// window's webview, at the default style so it follows the system's Liquid
/// Glass appearance setting. Returns false when the class is unavailable (older
/// macOS) so the caller can fall back to NSVisualEffectView vibrancy. Must run
/// on the main thread (we call it from `setup`). Validated in
/// `examples/glass_check.rs`.
#[cfg(target_os = "macos")]
fn apply_liquid_glass(win: &WebviewWindow, radius: f64) -> bool {
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2::{msg_send, sel};
    let Some(cls) = AnyClass::get("NSGlassEffectView") else {
        return false;
    };
    let Ok(ns_win) = win.ns_window() else {
        return false;
    };
    unsafe {
        let ns_win = ns_win as *mut AnyObject;
        let content: *mut AnyObject = msg_send![ns_win, contentView];
        if content.is_null() {
            return false;
        }
        let bounds: objc2_foundation::NSRect = msg_send![content, bounds];
        let glass: *mut AnyObject = msg_send![cls, alloc];
        let glass: *mut AnyObject = msg_send![glass, initWithFrame: bounds];
        // NSViewWidthSizable | NSViewHeightSizable — track window resizes (the
        // popover self-sizes to its content height).
        let _: () = msg_send![glass, setAutoresizingMask: 18u64];
        let _: () = msg_send![glass, setCornerRadius: radius];
        // Use the private "Widgets" material variant (4) — the same material the
        // desktop widgets use, verified in examples/glass_check.rs to be markedly
        // more transparent than the default (`.regular`) style, which read milky
        // over light backdrops. `set_variant:` is private, so guard on it and
        // fall back to the default material if it ever vanishes (cosmetic only).
        // We deliberately don't force `setStyle:` so the glass still tracks
        // System Settings → Appearance → Liquid Glass (Clear/Tinted).
        let responds: Bool = msg_send![glass, respondsToSelector: sel!(set_variant:)];
        if responds.as_bool() {
            let _: () = msg_send![glass, set_variant: 4i64];
            eprintln!("[hpbar] glass variant 4 (Widgets) applied");
        } else {
            eprintln!("[hpbar] set_variant: unavailable — default glass material");
        }
        // Behind the WKWebView: bottom of the content view's sibling stack.
        // Always visible: the glass IS the unfocused "widget" look (the CSS
        // panel goes near-solid over it while focused).
        let nil: *mut AnyObject = std::ptr::null_mut();
        let _: () = msg_send![content, addSubview: glass, positioned: -1isize, relativeTo: nil];
        // Remember it so the Settings "Liquid Glass" toggle can hide/show it.
        GLASS_VIEW.store(glass as usize, Ordering::Relaxed);
    }

    // Keep the glass from adopting its cloudy "inactive" look when the popover
    // isn't the key window — that's the whole point of the unfocused state. See
    // force_key_window_appearance; verified in examples/glass_check.rs (A/B).
    force_key_window_appearance(win);
    true
}

/// Override `hasKeyAppearance` -> YES *in place* on the popover window's class,
/// so its NSGlassEffectView keeps the clear "key" appearance even while the
/// window isn't key — the way always-active windows / desktop-widget hosts
/// render. Without this the glass clouds grey the instant the app deactivates
/// (the opposite of the widget look we want when unfocused).
///
/// `hasKeyAppearance` is public NSWindow API; we only override its getter, and
/// do it via class_addMethod on the existing class (no reparenting → no
/// super-chain recursion, unlike object_setClass which stack-overflowed).
/// Applies once, class-wide; harmless for the app's other (non-glass) windows.
/// The Focused *event* is unaffected, so the CSS focus state still toggles.
#[cfg(target_os = "macos")]
fn force_key_window_appearance(win: &WebviewWindow) {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
    use std::sync::OnceLock;
    static DONE: OnceLock<()> = OnceLock::new();
    let Ok(ns_win) = win.ns_window() else {
        return;
    };
    let ns_win = ns_win as *mut AnyObject;
    if ns_win.is_null() {
        return;
    }
    DONE.get_or_init(|| unsafe {
        let cls: *const AnyClass = msg_send![ns_win, class];
        extern "C" fn yes(_this: *mut AnyObject, _cmd: Sel) -> Bool {
            Bool::YES
        }
        let imp: unsafe extern "C" fn() =
            std::mem::transmute(yes as extern "C" fn(*mut AnyObject, Sel) -> Bool);
        // "c@:" — returns BOOL(char), args self + _cmd.
        let sel = objc2::ffi::sel_registerName(c"hasKeyAppearance".as_ptr());
        let added = objc2::ffi::class_addMethod(
            cls as *mut objc2::ffi::objc_class,
            sel,
            Some(imp),
            c"c@:".as_ptr(),
        );
        // class_addMethod returns the raw ObjC BOOL, which is `bool` on arm64 but
        // `signed char` (i8) on x86_64 — so `!added` only compiles on Apple Silicon.
        // Bool::from_raw normalises both to a portable Rust bool (fixes the
        // universal-build x86_64 slice).
        if !Bool::from_raw(added).as_bool() {
            eprintln!("[hpbar] hasKeyAppearance override not added");
        }
    });
}

fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    // The tray heart is a *live colour gauge* driven by `ambient`, so it must not
    // be a macOS template (template mode renders alpha-only, discarding our
    // colours). Start with a neutral blue-grey heart — visible on light *and* dark
    // bars — which the first quota poll recolours to your remaining HP.
    let (rgba, w, h) = heart_icon::render_neutral(6);
    let icon = Image::new_owned(rgba, w, h);

    #[allow(unused_mut)]
    let mut builder = TrayIconBuilder::with_id("main")
        .icon(icon)
        .icon_as_template(false)
        .tooltip("HPBar");

    // macOS gets NO native tray menu. Since macOS 26 (Tahoe) a status item with a
    // bound menu shows that menu on *left* click, stealing the click meant to open
    // the popover — and there's no API to keep a menu without binding it natively
    // (still true in tray-icon 0.24.1). So on macOS the menu's items live in the
    // popover's Settings (`get_app_controls` / `set_*` / `quit_app`) and *both*
    // tray clicks open the popover. Linux/Windows keep the native menu: Linux
    // never reports clicks (a menu item is the only way in), and Windows uses the
    // right-click menu with left-click reserved for the popover.
    #[cfg(not(target_os = "macos"))]
    {
        let launch_enabled = app.autolaunch().is_enabled().unwrap_or(false);
        let autostart = CheckMenuItem::with_id(
            app,
            "autostart",
            "Open at Login",
            true,
            launch_enabled,
            None::<&str>,
        )?;
        // Ambient-HP low/critical quota notifications; persisted, defaults on.
        let settings = ambient::load_settings(app);
        let alerts = CheckMenuItem::with_id(
            app,
            "alerts",
            "Quota Alerts",
            true,
            settings.alerts_enabled,
            None::<&str>,
        )?;
        // Device-share calibration: assert this is the only active device so the
        // fit trusts current usage as the single-device floor. Persisted, off.
        let calib = CheckMenuItem::with_id(
            app,
            "calib",
            "Only Device Here",
            true,
            settings.only_active_device,
            None::<&str>,
        )?;
        let quit = MenuItem::with_id(app, "quit", "Quit HPBar", true, None::<&str>)?;
        // Linux trays (libappindicator/StatusNotifierItem) don't deliver
        // left-click events, so a menu item is the only reliable way to open the
        // popover there.
        let show = MenuItem::with_id(app, "show", "Show HPBar", true, None::<&str>)?;
        let menu = Menu::with_items(
            app,
            &[
                &show,
                &PredefinedMenuItem::separator(app)?,
                &autostart,
                &alerts,
                &calib,
                &PredefinedMenuItem::separator(app)?,
                &quit,
            ],
        )?;
        // Captured so the toggle handlers reflect the new state in the checkmark.
        let autostart_item = autostart.clone();
        let alerts_item = alerts.clone();
        let calib_item = calib.clone();

        // Linux: menu on left click (see above). Windows: menu on right click,
        // left-click reserved for the popover.
        #[cfg(target_os = "linux")]
        let show_menu_on_left_click = true;
        #[cfg(not(target_os = "linux"))]
        let show_menu_on_left_click = false;

        builder = builder
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
                "alerts" => {
                    let mut s = ambient::load_settings(app);
                    s.alerts_enabled = !s.alerts_enabled;
                    ambient::save_settings(app, &s);
                    let _ = alerts_item.set_checked(s.alerts_enabled);
                }
                "calib" => {
                    let mut s = ambient::load_settings(app);
                    s.only_active_device = !s.only_active_device;
                    ambient::save_settings(app, &s);
                    let _ = calib_item.set_checked(s.only_active_device);
                }
                _ => {}
            });
    }

    builder
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                // macOS has no native menu, so both buttons open the popover.
                // Elsewhere right-click is the native menu, so only left opens it.
                let opens_popover = button == MouseButton::Left
                    || (cfg!(target_os = "macos") && button == MouseButton::Right);
                if opens_popover {
                    if let Some(win) = tray.app_handle().get_webview_window("popover") {
                        toggle_popover(&win, rect);
                    }
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
/// works on Linux, where the tray backend never emits left-click events. macOS
/// has no tray menu (both clicks are anchored), so this is unused there.
#[cfg(not(target_os = "macos"))]
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
/// have the icon's own rectangle to position against. Unused on macOS (no
/// menu-driven, unanchored open there).
#[allow(dead_code)]
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
#[allow(dead_code)]
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
