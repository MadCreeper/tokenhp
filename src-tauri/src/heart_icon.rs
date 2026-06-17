//! Renders the live-HP heart drawn into the tray / menu-bar icon.
//!
//! HPBar's whole pitch is "your usage as a health bar in the menu bar" — but the
//! tray icon was a *static* heart, so you had to click to see anything. This
//! turns the icon itself into the gauge: one heart that fills bottom-up to your
//! remaining-quota fraction and shifts colour by zone (healthy → caution → low →
//! critical), so the menu bar conveys your quota at a glance.
//!
//! Pure raster (no fonts, no SVG, no extra runtime deps): we reuse the exact 7×7
//! pixel-heart grid from the popover (`src/hearts.ts`) and upscale it, so the
//! tray and the panel always show the same heart.

/// 7×7 heart. Codes: 0 transparent · 1 outline · 2 body · 3 sparkle. Identical to
/// `PATTERN` in `src/hearts.ts`.
const PATTERN: [[u8; 7]; 7] = [
    [0, 1, 1, 0, 1, 1, 0],
    [1, 2, 2, 1, 2, 2, 1],
    [1, 3, 2, 2, 2, 2, 1],
    [1, 2, 2, 2, 2, 2, 1],
    [0, 1, 2, 2, 2, 1, 0],
    [0, 0, 1, 2, 1, 0, 0],
    [0, 0, 0, 1, 0, 0, 0],
];

const GRID: u32 = 7;

pub type Rgb = (u8, u8, u8);

/// Colours for one heart render: the lit (filled) pixels and the drained (empty)
/// pixels, split into body / sparkle / outline so the upscaled heart keeps the
/// Minecraft shading.
#[derive(Clone, Copy)]
pub struct HeartStyle {
    pub full_body: Rgb,
    pub full_sparkle: Rgb,
    pub full_outline: Rgb,
    pub empty_body: Rgb,
    pub empty_sparkle: Rgb,
    pub empty_outline: Rgb,
}

/// Scale a colour toward black by factor `f` (0 → black, 1 → unchanged).
fn darken(c: Rgb, f: f64) -> Rgb {
    let m = |x: u8| (x as f64 * f).round().clamp(0.0, 255.0) as u8;
    (m(c.0), m(c.1), m(c.2))
}

/// Health-ramp tint for the heart, by remaining fraction: green → amber → orange
/// → red (matching the app's "Classic" HP theme). The *drained* pixels are dark
/// versions of the same hue (the Minecraft empty-heart look), so the fill level
/// reads from **luminance** — a bright-vs-dark contrast that survives the menu
/// bar's vibrancy, which can wash the *hue* out entirely (a red can render as a
/// muddy blue over a blue wallpaper). Colour is then a bonus where it survives;
/// the precise level falls back to the tray title (see `ambient`) when low.
pub fn zone_style(remaining: f64) -> HeartStyle {
    let (body, sparkle, outline) = if remaining <= 0.10 {
        ((0xff, 0x2a, 0x22), (0xff, 0xc4, 0xc0), (0x7a, 0x0c, 0x08)) // critical · red
    } else if remaining <= 0.25 {
        ((0xff, 0x7a, 0x10), (0xff, 0xdc, 0xae), (0x7c, 0x38, 0x00)) // low · orange
    } else if remaining <= 0.50 {
        ((0xff, 0xc4, 0x00), (0xff, 0xf2, 0xbf), (0x6e, 0x52, 0x00)) // caution · amber
    } else {
        ((0x32, 0xcf, 0x52), (0xc6, 0xff, 0xd2), (0x0e, 0x60, 0x26)) // healthy · green
    };
    HeartStyle {
        full_body: body,
        full_sparkle: sparkle,
        full_outline: outline,
        empty_body: darken(body, 0.30),
        empty_sparkle: darken(sparkle, 0.26),
        empty_outline: darken(outline, 0.6),
    }
}

fn pixel(code: u8, lit: bool, s: &HeartStyle) -> Option<Rgb> {
    match (code, lit) {
        (1, true) => Some(s.full_outline),
        (1, false) => Some(s.empty_outline),
        (2, true) => Some(s.full_body),
        (2, false) => Some(s.empty_body),
        (3, true) => Some(s.full_sparkle),
        (3, false) => Some(s.empty_sparkle),
        _ => None, // code 0 → transparent
    }
}

/// Render the heart at `remaining` (0..1) with an explicit style, upscaled by
/// `scale`. Returns `(rgba, width, height)` — straight RGBA8, transparent where
/// the heart isn't. The fill is evaluated per *output* pixel row, so the level is
/// smooth even though the heart shape is chunky.
pub fn render_rgba_styled(remaining: f64, scale: u32, style: &HeartStyle) -> (Vec<u8>, u32, u32) {
    let scale = scale.max(1);
    let w = GRID * scale;
    let h = GRID * scale;
    let r = remaining.clamp(0.0, 1.0);
    // Rows at or below this line are lit; 0 ⇒ all lit (full), h ⇒ none (empty).
    let fill_line = (1.0 - r) * h as f64;

    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for oy in 0..h {
        let lit = (oy as f64) >= fill_line;
        let cy = (oy / scale) as usize;
        for ox in 0..w {
            let cx = (ox / scale) as usize;
            let Some((cr, cg, cb)) = pixel(PATTERN[cy][cx], lit, style) else {
                continue; // leave transparent
            };
            let i = ((oy * w + ox) * 4) as usize;
            rgba[i] = cr;
            rgba[i + 1] = cg;
            rgba[i + 2] = cb;
            rgba[i + 3] = 0xff;
        }
    }
    (rgba, w, h)
}

/// Render with the default health-ramp zone style for `remaining`.
pub fn render_rgba(remaining: f64, scale: u32) -> (Vec<u8>, u32, u32) {
    render_rgba_styled(remaining, scale, &zone_style(remaining))
}

/// A uniform muted blue-grey heart for the "no data yet" / signed-out state. Used
/// as the tray's initial icon so it's visible (and *not* a template) before the
/// first quota poll lands.
pub fn neutral_style() -> HeartStyle {
    let body: Rgb = (0x8a, 0x95, 0xa3);
    let sparkle: Rgb = (0xb4, 0xbe, 0xc9);
    let outline: Rgb = (0x55, 0x5e, 0x6a);
    HeartStyle {
        full_body: body,
        full_sparkle: sparkle,
        full_outline: outline,
        empty_body: body,
        empty_sparkle: sparkle,
        empty_outline: outline,
    }
}

/// Full neutral heart (signed-out / pre-data state).
pub fn render_neutral(scale: u32) -> (Vec<u8>, u32, u32) {
    render_rgba_styled(1.0, scale, &neutral_style())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimensions_and_alpha() {
        let (rgba, w, h) = render_rgba(0.5, 6);
        assert_eq!(w, 42);
        assert_eq!(h, 42);
        assert_eq!(rgba.len(), (42 * 42 * 4) as usize);
        // The very top-left cell (code 0) must be transparent.
        assert_eq!(rgba[3], 0);
    }

    #[test]
    fn full_is_more_lit_than_empty() {
        // Distinct lit/empty colours so the count is unambiguous.
        let style = HeartStyle {
            full_body: (0, 255, 0),
            full_sparkle: (0, 255, 0),
            full_outline: (0, 255, 0),
            empty_body: (10, 10, 10),
            empty_sparkle: (10, 10, 10),
            empty_outline: (10, 10, 10),
        };
        let lit_count = |r: f64| {
            let (rgba, w, h) = render_rgba_styled(r, 6, &style);
            (0..(w * h))
                .filter(|i| {
                    let p = (*i * 4) as usize;
                    rgba[p + 3] == 0xff && rgba[p + 1] == 255 && rgba[p] == 0
                })
                .count()
        };
        assert!(lit_count(1.0) > lit_count(0.2), "full heart should have more lit pixels");
    }

    #[test]
    fn empty_heart_still_has_silhouette() {
        // At 0% the heart must still be drawn (grey), so the icon stays findable.
        let (rgba, w, h) = render_rgba(0.0, 6);
        let opaque = (0..(w * h)).filter(|i| rgba[(*i * 4 + 3) as usize] == 0xff).count();
        assert!(opaque > 200, "drained heart should still render a visible silhouette");
    }
}
