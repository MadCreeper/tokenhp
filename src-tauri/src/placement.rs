//! Physical-pixel popup geometry, independent of the window system for tests.
#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Stay outside the taskbar/menu bar and inside the monitor's usable area.
/// Insets identify top/bottom/side taskbars, including secondary monitors.
pub fn anchored(icon: Bounds, work: Bounds, size: (f64, f64), scale: f64) -> (f64, f64) {
    let gap = 6.0 * scale;
    let (width, height) = size;
    let right = work.x + work.width;
    let bottom = work.y + work.height;
    let (x, y) = if icon.y >= bottom - gap {
        (icon.x + icon.width - width, bottom - height - gap)
    } else if icon.y + icon.height <= work.y + gap {
        (icon.x + icon.width - width, work.y + gap)
    } else if icon.x + icon.width <= work.x + gap {
        (work.x + gap, icon.y + icon.height - height)
    } else if icon.x >= right - gap {
        (right - width - gap, icon.y + icon.height - height)
    } else if icon.y + icon.height / 2.0 > work.y + work.height / 2.0 {
        (icon.x + icon.width - width, icon.y - height - gap)
    } else {
        (icon.x + icon.width - width, icon.y + icon.height + gap)
    };
    (
        x.clamp(work.x, (right - width).max(work.x)),
        y.clamp(work.y, (bottom - height).max(work.y)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bottom_taskbar_reanchors_after_growth_at_high_dpi() {
        let work = Bounds {
            x: 1920.,
            y: 0.,
            width: 2560.,
            height: 1360.,
        };
        let icon = Bounds {
            x: 4340.,
            y: 1370.,
            width: 32.,
            height: 32.,
        };
        let small = anchored(icon, work, (540., 450.), 1.5);
        let large = anchored(icon, work, (540., 900.), 1.5);
        assert_eq!(small.1 + 450., large.1 + 900.);
        assert_eq!(large, (3832., 451.));
    }
    #[test]
    fn negative_monitors_top_and_side_taskbars_and_oversized_windows() {
        let work = Bounds {
            x: -1880.,
            y: -100.,
            width: 1880.,
            height: 1080.,
        };
        for icon in [
            Bounds {
                x: -1910.,
                y: 300.,
                width: 25.,
                height: 25.,
            },
            Bounds {
                x: -40.,
                y: 300.,
                width: 30.,
                height: 30.,
            },
            Bounds {
                x: -50.,
                y: -140.,
                width: 25.,
                height: 25.,
            },
        ] {
            let (x, y) = anchored(icon, work, (360., 600.), 1.);
            assert!(x >= work.x && x + 360. <= work.x + work.width);
            assert!(y >= work.y && y + 600. <= work.y + work.height);
        }
        assert_eq!(anchored(work, work, (3000., 2000.), 1.), (work.x, work.y));
    }
}
