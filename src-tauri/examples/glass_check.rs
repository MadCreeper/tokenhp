//! Native "liquid glass" check — opens one borderless window per
//! NSVisualEffectMaterial so you can judge the REAL macOS material (no CSS
//! simulation, no dev server, doesn't touch the main app).
//!
//!   cargo run --example glass_check
//!
//! Each window: native NSVisualEffectView behind a transparent webview, with a
//! mini classic-theme panel + a tint-alpha slider on top. The page is served via
//! a custom `glass://` URI scheme — the one Tauri-supported way to hand raw HTML
//! to a webview with a response we fully control (data: URLs get the app CSP
//! injected, which blanks the page; about:blank ignores init scripts).
//! Ctrl+C to quit.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("glass_check is macOS-only (NSVisualEffectView).");
}

#[cfg(target_os = "macos")]
static GLASS_PROBE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Override `hasKeyAppearance` -> YES *in place* on the window's class, so any
/// NSGlassEffectView inside keeps its clear "key" look even when the window is
/// not key (how always-active windows / widget hosts render). Uses
/// class_addMethod on the existing class (all our windows share tao's window
/// class) — no reparenting, so no super-chain recursion. Runs once.
#[cfg(target_os = "macos")]
fn force_key_appearance(ns_win: *mut objc2::runtime::AnyObject) {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
    use objc2::sel;
    use std::sync::OnceLock;
    static DONE: OnceLock<()> = OnceLock::new();
    if ns_win.is_null() {
        return;
    }
    DONE.get_or_init(|| unsafe {
        let cls: *const AnyClass = msg_send![ns_win, class];
        extern "C" fn yes(_this: *mut AnyObject, _cmd: Sel) -> Bool {
            Bool::YES
        }
        // "c@:" = returns BOOL(char), args self + _cmd.
        let types = c"c@:";
        let imp: unsafe extern "C" fn() =
            std::mem::transmute(yes as extern "C" fn(*mut AnyObject, Sel) -> Bool);
        let sel_ptr = objc2::ffi::sel_registerName(c"hasKeyAppearance".as_ptr());
        let added = objc2::ffi::class_addMethod(
            cls as *mut objc2::ffi::objc_class,
            sel_ptr,
            Some(imp),
            types.as_ptr(),
        );
        eprintln!("[glass_check] hasKeyAppearance override added = {added:?}");
        let _ = sel!(hasKeyAppearance); // keep the import used if sel! is elsewhere
    });
}

#[cfg(target_os = "macos")]
fn main() {
    use std::sync::atomic::Ordering;
    use tauri::{Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
    use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};

    // The materials worth comparing for a popover-style panel. On macOS 26 the
    // light materials largely converge to the new glass look, so the big visual
    // fork is the window appearance: the `dark: true` entries force DarkAqua to
    // show the dark glass variant of the same material.
    let materials: &[(&str, NSVisualEffectMaterial, bool)] = &[
        ("Popover", NSVisualEffectMaterial::Popover, false),
        ("Popover-dark", NSVisualEffectMaterial::Popover, true),
        ("HudWindow", NSVisualEffectMaterial::HudWindow, false),
        ("HudWindow-dark", NSVisualEffectMaterial::HudWindow, true),
        ("Sidebar", NSVisualEffectMaterial::Sidebar, false),
    ];

    // Same fix as build_popover in lib.rs: wry disables drawsBackground for a
    // transparent webview but leaves the opaque macOS-12+ underPageBackgroundColor,
    // which paints white over the vibrancy.
    fn clear_webview_background(win: &WebviewWindow, name: &'static str) {
        let r = win.with_webview(move |webview| {
            use objc2::runtime::{AnyObject, Bool};
            use objc2::{class, msg_send, sel};
            unsafe {
                let wk = webview.inner() as *mut AnyObject;
                if wk.is_null() {
                    eprintln!("[glass_check] {name}: webview inner() null");
                    return;
                }
                let _: () = msg_send![wk, setOpaque: Bool::new(false)];
                let responds: Bool =
                    msg_send![wk, respondsToSelector: sel!(setUnderPageBackgroundColor:)];
                if responds.as_bool() {
                    let clear: *mut AnyObject = msg_send![class!(NSColor), clearColor];
                    let _: () = msg_send![wk, setUnderPageBackgroundColor: clear];
                }
                eprintln!("[glass_check] {name}: webview background cleared");
            }
        });
        if let Err(e) = r {
            eprintln!("[glass_check] {name}: with_webview failed: {e:?}");
        }
    }

    tauri::Builder::default()
        // Serve the demo page ourselves: glass://localhost/<MaterialName>.
        // Full control of the response — no CSP injection, no dev server.
        .register_uri_scheme_protocol("glass", |_ctx, request| {
            let material = request.uri().path().trim_start_matches('/').to_string();
            tauri::http::Response::builder()
                .header("Content-Type", "text/html; charset=utf-8")
                .body(page_html(&material).into_bytes())
                .expect("response")
        })
        .setup(move |app| {
            for (i, (name, material, dark)) in materials.iter().enumerate() {
                let win = WebviewWindowBuilder::new(
                    app,
                    format!("glass{i}"),
                    WebviewUrl::External(
                        format!("glass://localhost/{name}").parse().expect("url"),
                    ),
                )
                .title(*name)
                .inner_size(240.0, 380.0)
                // 4 per row so all windows fit on a laptop display.
                .position(
                    30.0 + (i % 4) as f64 * 258.0,
                    60.0 + (i / 4) as f64 * 400.0,
                )
                .resizable(false)
                .decorations(false)
                .always_on_top(true)
                .transparent(true)
                .build()?;

                // Force DarkAqua on the marked windows so both glass variants of
                // the material are visible side by side (setup runs on the main
                // thread, so touching the NSWindow directly here is safe).
                if *dark {
                    use objc2::runtime::AnyObject;
                    use objc2::{class, msg_send};
                    unsafe {
                        let ns_win = win.ns_window()? as *mut AnyObject;
                        let cname = std::ffi::CString::new("NSAppearanceNameDarkAqua").unwrap();
                        let ns_name: *mut AnyObject =
                            msg_send![class!(NSString), stringWithUTF8String: cname.as_ptr()];
                        let appearance: *mut AnyObject =
                            msg_send![class!(NSAppearance), appearanceNamed: ns_name];
                        let _: () = msg_send![ns_win, setAppearance: appearance];
                    }
                }

                match apply_vibrancy(&win, *material, Some(NSVisualEffectState::Active), Some(12.0))
                {
                    Ok(()) => eprintln!("[glass_check] {name}: vibrancy applied"),
                    Err(e) => eprintln!("[glass_check] {name}: vibrancy FAILED: {e:?}"),
                }
                clear_webview_background(&win, name);
            }

            // Bottom row: REAL Liquid Glass (NSGlassEffectView, macOS 26+), one
            // window per look (runtime-introspected on macOS 27):
            //   setStyle:      PUBLIC  — 0 regular (frosted), 1 clear (CC lens)
            //   set_variant:   private — material variants (8 = Control Center)
            // Glass-System sets NEITHER: the material then follows the user's
            // System Settings → Appearance → Liquid Glass (Clear/Tinted) choice,
            // which is where the system-wide "frostiness" lives — flip it and
            // watch that window change. Forcing a style overrides that setting.
            // Radius 26: the signature lensing lives in the corner curvature and
            // is invisible at 12.
            let variants: &[(&str, Option<i64>, Option<i64>)] = &[
                ("Glass-System", None, None),
                ("Glass-Clear", Some(1), None),
                ("Glass-Widgets", None, Some(4)),
                ("Glass-ControlCenter", None, Some(8)),
            ];
            for (j, (name, style, variant)) in variants.iter().enumerate() {
                let win = WebviewWindowBuilder::new(
                    app,
                    format!("glassv{j}"),
                    WebviewUrl::External(
                        format!("glass://localhost/{name}").parse().expect("url"),
                    ),
                )
                .title(*name)
                .inner_size(240.0, 380.0)
                // Over the wallpaper (center of screen), spread out, so the
                // material — not the backdrop — is what differs between them.
                .position(560.0 + j as f64 * 270.0, 360.0)
                .resizable(false)
                .decorations(false)
                .always_on_top(true)
                .transparent(true)
                .build()?;

                use objc2::runtime::{AnyClass, AnyObject, Bool};
                use objc2::{class, msg_send, sel};
                unsafe {
                    match AnyClass::get("NSGlassEffectView") {
                        Some(cls) => {
                            let ns_win = win.ns_window()? as *mut AnyObject;
                            let content: *mut AnyObject = msg_send![ns_win, contentView];
                            let bounds: objc2_foundation::NSRect = msg_send![content, bounds];
                            let g: *mut AnyObject = msg_send![cls, alloc];
                            let g: *mut AnyObject = msg_send![g, initWithFrame: bounds];
                            // NSViewWidthSizable | NSViewHeightSizable
                            let _: () = msg_send![g, setAutoresizingMask: 18u64];
                            let _: () = msg_send![g, setCornerRadius: 26.0f64];
                            if let Some(style) = style {
                                let responds: Bool =
                                    msg_send![g, respondsToSelector: sel!(setStyle:)];
                                if responds.as_bool() {
                                    let _: () = msg_send![g, setStyle: *style];
                                    eprintln!("[glass_check] {name}: style {style} set");
                                } else {
                                    eprintln!("[glass_check] {name}: no setStyle:");
                                }
                            }
                            if let Some(variant) = variant {
                                let responds: Bool =
                                    msg_send![g, respondsToSelector: sel!(set_variant:)];
                                if responds.as_bool() {
                                    let _: () = msg_send![g, set_variant: *variant];
                                    eprintln!("[glass_check] {name}: variant {variant} set");
                                } else {
                                    eprintln!("[glass_check] {name}: no set_variant:");
                                }
                            }
                            // Behind the WKWebView: bottom of the sibling stack.
                            let nil: *mut AnyObject = std::ptr::null_mut();
                            let _: () = msg_send![content, addSubview: g, positioned: -1isize, relativeTo: nil];
                            eprintln!("[glass_check] {name}: NSGlassEffectView applied");
                            // Override hasKeyAppearance -> YES on the window class
                            // (affects all our windows) so the glass never adopts
                            // the cloudy non-key look. Triggered from Glass-Widgets;
                            // applies class-wide, so every glass window should stay
                            // clear when the app is inactive.
                            if *name == "Glass-Widgets" {
                                force_key_appearance(ns_win);
                            }
                        }
                        None => eprintln!(
                            "[glass_check] {name}: NSGlassEffectView unavailable on this macOS"
                        ),
                    }
                }
                clear_webview_background(&win, name);
            }
            eprintln!("[glass_check] 8 windows up — Ctrl+C here to quit");

            // With GLASS_FORCE_ACTIVE=1: keep re-asserting the private
            // `_setHasActiveAppearance:YES` on the probed window every second,
            // so the backdrop never adopts the frosted-grey inactive look even
            // while the app is deactivated. (Re-asserted, not set once — AppKit
            // rewrites the flag on every app active/inactive transition.)
            if std::env::var_os("GLASS_FORCE_ACTIVE").is_some() {
                let handle = app.handle().clone();
                std::thread::spawn(move || loop {
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    let _ = handle.run_on_main_thread(|| {
                        use objc2::msg_send;
                        use objc2::runtime::{AnyObject, Bool};
                        use objc2::sel;
                        let p = GLASS_PROBE.load(Ordering::Relaxed);
                        if p == 0 {
                            return;
                        }
                        unsafe {
                            let w = p as *mut AnyObject;
                            let responds: Bool = msg_send![w, respondsToSelector: sel!(acquireKeyAppearance)];
                            if responds.as_bool() {
                                let _: () = msg_send![w, acquireKeyAppearance];
                            } else {
                                eprintln!("[probe] acquireKeyAppearance unavailable");
                            }
                        }
                    });
                });
                eprintln!("[glass_check] forcing active appearance on Glass-Clear window");
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("glass_check run");
}

/// Minimal classic-theme panel over a transparent page, plus a slider that
/// changes the panel tint alpha live (pure in-page JS, no Tauri API needed).
/// The panel fills the window (no gutter — bare glass showing around the edge
/// read as an untintable "border"), and the slider drives the border and inset
/// highlight along with the fill. `*-dark` windows get a dark tint + light text.
#[cfg(target_os = "macos")]
fn page_html(material: &str) -> String {
    let dark = material.ends_with("-dark");
    // Panel tint base and chrome per appearance.
    let (rgb, text, dim, track) = if dark {
        ("30,30,34", "#f2f2f5", "#b7b7bd", "rgba(255,255,255,.25)")
    } else {
        ("245,245,248", "#2b2b2e", "#6e6e73", "rgba(0,0,0,.25)")
    };
    // Glass windows use CC-style curvature (must match setCornerRadius above),
    // and a minimal default tint: the native material supplies the frostiness
    // (per the system's Liquid Glass appearance setting) — CSS alpha is only a
    // text-contrast assist on top, not the source of the look.
    let glass = material.starts_with("Glass");
    let radius = if glass { 26 } else { 12 };
    let alpha = if glass { "0.10" } else { "0.30" };
    let alpha_pct = if glass { 10 } else { 30 };
    // Border/highlight are white in both appearances; only their alpha scales.
    format!(
        r##"<!doctype html><html><head><meta charset="utf-8"><style>
html,body{{margin:0;height:100%;background:transparent;overflow:hidden;
  font-family:-apple-system,"SF Pro Text",sans-serif;-webkit-user-select:none;cursor:default}}
.panel{{height:100%;box-sizing:border-box;border-radius:{radius}px;padding:14px;color:{text}}}
h1{{font-size:13px;margin:0}}
.mat{{font-size:10.5px;color:{dim};margin:2px 0 14px;font-family:ui-monospace,monospace}}
.t{{display:flex;justify-content:space-between;font-size:11px;font-weight:600;margin-top:12px}}
.t span:last-child{{color:{dim};font-weight:400}}
.track{{height:10px;border-radius:3px;background:{track};overflow:hidden;margin-top:4px}}
.fill{{height:100%;border-radius:3px}}
label{{font-size:10px;color:{dim};display:block;margin-top:18px}}
input{{width:100%}}
</style></head><body>
<div class="panel" id="p" data-tauri-drag-region>
  <h1>&#9829; HP Bar</h1>
  <div class="mat">{material}</div>
  <div class="t"><span>Current session</span><span>62%</span></div>
  <div class="track"><div class="fill" style="width:62%;background:linear-gradient(to bottom,#6fce72,#4caf50)"></div></div>
  <div class="t"><span>Weekly &middot; all models</span><span>41%</span></div>
  <div class="track"><div class="fill" style="width:41%;background:linear-gradient(to bottom,#eec14a,#e0a92e)"></div></div>
  <div class="t"><span>Weekly &middot; Opus</span><span>18%</span></div>
  <div class="track"><div class="fill" style="width:18%;background:linear-gradient(to bottom,#e6685c,#d9584a)"></div></div>
  <label>panel tint alpha <span id="v">{alpha}</span></label>
  <input id="a" type="range" min="0" max="100" value="{alpha_pct}">
</div>
<script>
var p=document.getElementById('p'),a=document.getElementById('a'),v=document.getElementById('v');
// Specular rim like the native material: a floor alpha keeps the edge visible
// even at zero tint (real glass still catches light), scaling up with the tint.
function set(x){{v.textContent=x.toFixed(2);
  p.style.background='rgba({rgb},'+x+')';
  p.style.border='1px solid rgba(255,255,255,'+(0.22+0.6*x)+')';
  p.style.boxShadow='inset 0 1px 1px rgba(255,255,255,'+(0.3+0.7*x)+'),'+
    'inset 0 -1px 1px rgba(255,255,255,'+(0.12+0.2*x)+')'}}
a.oninput=function(){{set(a.value/100)}};
set({alpha});
</script></body></html>"##
    )
}
