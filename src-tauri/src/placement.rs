//! Popup geometry in one consistent coordinate space (pixels on Windows/Linux,
//! logical points on macOS), independent of the window system for tests.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

pub fn contains(bounds: Bounds, point: (f64, f64)) -> bool {
    point.0 >= bounds.x
        && point.0 < bounds.x + bounds.width
        && point.1 >= bounds.y
        && point.1 < bounds.y + bounds.height
}

pub fn distance_squared(bounds: Bounds, point: (f64, f64)) -> f64 {
    let dx = point.0 - point.0.clamp(bounds.x, bounds.x + bounds.width);
    let dy = point.1 - point.1.clamp(bounds.y, bounds.y + bounds.height);
    dx * dx + dy * dy
}

/// Recover an icon's logical rectangle relative to the actual native click.
/// Stale rectangles from replicated menu bars fall back to centering on the click.
pub fn tray_click_anchor(
    cursor: (f64, f64),
    event_rect: Bounds,
    event_position: (f64, f64),
    scale: f64,
) -> Bounds {
    let width = (event_rect.width / scale).clamp(1.0, 120.0);
    let height = (event_rect.height / scale).clamp(1.0, 48.0);
    let (dx, dy) = if contains(event_rect, event_position) {
        (
            (event_position.0 - event_rect.x) / scale,
            (event_position.1 - event_rect.y) / scale,
        )
    } else {
        (width / 2.0, height / 2.0)
    };
    Bounds {
        x: cursor.0 - dx,
        y: cursor.1 - dy,
        width,
        height,
    }
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
    fn three_displays_above_and_left_use_logical_points_at_every_scale() {
        // This machine: built-in screen, left display, upper display. Keep the
        // same logical layout while varying the clicked display's pixel scale.
        let displays = [
            Bounds {
                x: 0.,
                y: -956.,
                width: 1470.,
                height: 956.,
            },
            Bounds {
                x: -2223.,
                y: -1901.,
                width: 1680.,
                height: 945.,
            },
            Bounds {
                x: -543.,
                y: -2108.,
                width: 2048.,
                height: 1152.,
            },
        ];
        for work in displays {
            let cursor = (work.x + work.width - 80., work.y + 12.);
            for scale in [1., 1.5, 2.] {
                // Deliberately unrelated global pixel origin. Only the local
                // icon-to-click offset can be combined with native points.
                let event_rect = Bounds {
                    x: 1200. * scale,
                    y: 956. * scale,
                    width: 24. * scale,
                    height: 24. * scale,
                };
                let icon = tray_click_anchor(
                    cursor,
                    event_rect,
                    (event_rect.x + 12. * scale, event_rect.y + 12. * scale),
                    scale,
                );
                assert_eq!(icon.x, cursor.0 - 12.);
                assert_eq!(icon.y, work.y);
                for height in [300., 700.] {
                    let point = anchored(icon, work, (360., height), 1.);
                    assert_eq!(point, (cursor.0 + 12. - 360., work.y + 30.));
                    assert!(contains(work, point));
                    assert!(point.1 + height <= work.y + work.height);
                }
            }
        }
    }

    #[test]
    fn replicated_menu_bar_stale_rectangle_uses_actual_click() {
        let cursor = (-1800., -1889.);
        let icon = tray_click_anchor(
            cursor,
            Bounds {
                x: 2600.,
                y: 0.,
                width: 48.,
                height: 48.,
            },
            (-3600., -1866.),
            2.,
        );
        assert_eq!(
            (icon.x, icon.y, icon.width, icon.height),
            (-1812., -1901., 24., 24.)
        );
        let remaining = Bounds {
            x: 0.,
            y: -956.,
            width: 1470.,
            height: 956.,
        };
        let point = anchored(icon, remaining, (360., 300.), 1.);
        assert!(contains(remaining, point));
        assert!(distance_squared(remaining, cursor) > 0.);
    }

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
