//! Read back native window frames on every attached Mac display. No credentials
//! or network; the diagnostic window stays hidden. Run `cargo run --example placement_check`.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/mac_popup.rs"]
mod mac_popup;
#[cfg(target_os = "macos")]
#[path = "../src/placement.rs"]
mod placement;

#[cfg(target_os = "macos")]
fn main() {
    use objc2::{msg_send, runtime::AnyObject};
    use objc2_foundation::{NSRect, NSSize};
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    tauri::Builder::default()
        .setup(|app| {
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let window = WebviewWindowBuilder::new(
                app,
                "placement-check",
                WebviewUrl::External("about:blank".parse().unwrap()),
            )
            .decorations(false)
            .visible(false)
            .inner_size(360., 300.)
            .build()?;
            let screens = mac_popup::screens();
            assert!(!screens.is_empty());
            // Repeated moves exercise old-screen -> new-screen transitions, then a
            // content resize and re-anchor as happens during quota refresh/settings.
            for screen in screens.iter().chain(screens.iter().rev()) {
                for height in [300., 600.] {
                    let anchor = mac_popup::Anchor {
                        screen_id: screen.id,
                        icon: placement::Bounds {
                            x: screen.frame.x + screen.frame.width - 100.,
                            y: screen.frame.y,
                            width: 24.,
                            height: 24.,
                        },
                    };
                    unsafe {
                        let ptr = window.ns_window()? as *mut AnyObject;
                        let _: () = msg_send![ptr, setContentSize: NSSize::new(360., height)];
                        assert!(mac_popup::position(&window, anchor));
                        let frame: NSRect = msg_send![ptr, frame];
                        let point = (frame.origin.x, -(frame.origin.y + frame.size.height));
                        let expected =
                            placement::anchored(anchor.icon, screen.work, (360., height), 1.);
                        assert!(
                            (point.0 - expected.0).abs() < 1. && (point.1 - expected.1).abs() < 1.,
                            "screen {}: actual {:?}, expected {:?}",
                            screen.id,
                            point,
                            expected
                        );
                        assert!(mac_popup::window_on_screen(&window, screen.id));
                        println!(
                            "screen={} scale={} size=360x{} actual={:?} OK",
                            screen.id, screen.scale, height, point
                        );
                    }
                }
            }
            println!(
                "Native placement passed on {} displays in both directions.",
                screens.len()
            );
            app.handle().exit(0);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("placement diagnostic");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("placement_check uses AppKit; run on macOS");
}
