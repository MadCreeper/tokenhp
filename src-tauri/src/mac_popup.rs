//! AppKit-only popup placement. All coordinates are logical screen points.
//! Never mix per-display backing pixels into the global desktop coordinate space.
//! Call these functions on the main thread only.
use crate::placement::{self, Bounds};
use objc2::{class, msg_send, runtime::AnyObject};
use objc2_foundation::{NSPoint, NSRect};
use tauri::{PhysicalPosition, Rect, WebviewWindow};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Anchor {
    pub screen_id: u32,
    pub icon: Bounds,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Screen {
    pub id: u32,
    pub frame: Bounds,
    pub work: Bounds,
    pub scale: f64,
}

// Negate AppKit's y axis; no primary-monitor height or scale is needed.
fn bounds(rect: NSRect) -> Bounds {
    Bounds {
        x: rect.origin.x,
        y: -(rect.origin.y + rect.size.height),
        width: rect.size.width,
        height: rect.size.height,
    }
}

unsafe fn screen_id(screen: *mut AnyObject) -> u32 {
    let description: *mut AnyObject = msg_send![screen, deviceDescription];
    let key: *mut AnyObject =
        msg_send![class!(NSString), stringWithUTF8String: c"NSScreenNumber".as_ptr()];
    let number: *mut AnyObject = msg_send![description, objectForKey: key];
    msg_send![number, unsignedIntValue]
}

pub(crate) fn screens() -> Vec<Screen> {
    unsafe {
        let array: *mut AnyObject = msg_send![class!(NSScreen), screens];
        // Enumeration avoids signed-index ABI differences between NSArray
        // and the Swift-backed array returned by newer macOS releases.
        let iterator: *mut AnyObject = msg_send![array, objectEnumerator];
        let mut result = Vec::new();
        loop {
            let screen: *mut AnyObject = msg_send![iterator, nextObject];
            if screen.is_null() {
                break;
            }
            let frame: NSRect = msg_send![screen, frame];
            let work: NSRect = msg_send![screen, visibleFrame];
            let scale: f64 = msg_send![screen, backingScaleFactor];
            result.push(Screen {
                id: screen_id(screen),
                frame: bounds(frame),
                work: bounds(work),
                scale,
            });
        }
        result
    }
}

pub(crate) fn clicked_anchor(rect: Rect, event_position: PhysicalPosition<f64>) -> Option<Anchor> {
    let mouse: NSPoint = unsafe { msg_send![class!(NSEvent), mouseLocation] };
    let cursor = (mouse.x, -mouse.y);
    let screen = screens()
        .into_iter()
        .find(|s| placement::contains(s.frame, cursor))?;
    // Only use relative offsets from the tray event. Its global pixel origin
    // can differ from AppKit (and duplicated menu-bar icons can report a stale
    // rectangle). The native mouse position determines the actual display.
    let origin = rect.position.to_physical::<f64>(screen.scale);
    let size = rect.size.to_physical::<f64>(screen.scale);
    let icon = placement::tray_click_anchor(
        cursor,
        Bounds {
            x: origin.x,
            y: origin.y,
            width: size.width,
            height: size.height,
        },
        (event_position.x, event_position.y),
        screen.scale,
    );
    Some(Anchor {
        screen_id: screen.id,
        icon,
    })
}

pub(crate) fn window_on_screen(win: &WebviewWindow, id: u32) -> bool {
    let Ok(ptr) = win.ns_window() else {
        return false;
    };
    unsafe {
        let screen: *mut AnyObject = msg_send![ptr as *mut AnyObject, screen];
        !screen.is_null() && screen_id(screen) == id
    }
}

pub(crate) fn position(win: &WebviewWindow, anchor: Anchor) -> bool {
    let screens = screens();
    // A disconnected display must not strand the window off-screen. Retain the
    // clicked display by ID across layout/scale changes; otherwise use nearest.
    let screen = screens
        .iter()
        .find(|s| s.id == anchor.screen_id)
        .or_else(|| {
            screens.iter().min_by(|a, b| {
                placement::distance_squared(a.frame, (anchor.icon.x, anchor.icon.y)).total_cmp(
                    &placement::distance_squared(b.frame, (anchor.icon.x, anchor.icon.y)),
                )
            })
        });
    let Some(screen) = screen else {
        return false;
    };
    let Ok(ptr) = win.ns_window() else {
        return false;
    };
    unsafe {
        let window = ptr as *mut AnyObject;
        let frame: NSRect = msg_send![window, frame];
        let (x, y) = placement::anchored(
            anchor.icon,
            screen.work,
            (frame.size.width, frame.size.height),
            1.0,
        );
        // Set AppKit's top-left point directly. Tauri's physical-position setter
        // divides by the window's OLD screen scale before moving to a new one.
        let _: () = msg_send![window, setFrameTopLeftPoint: NSPoint::new(x, -y)];
    }
    true
}
