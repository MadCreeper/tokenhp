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

/// Filled-pixel count of each PATTERN row, top→bottom. The heart's area is
/// concentrated in its wide upper rows and tapers to a 1-pixel tip, so a fill
/// measured by row *height* badly under-reads: 25% remaining lights only the
/// narrow bottom ~3% of the area and the heart looks nearly empty. We use these
/// weights to place the fill line by *area* instead (see [`area_fill_line`]).
const ROW_WEIGHT: [u32; GRID as usize] = [4, 7, 7, 7, 5, 3, 1];

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

/// Which visual theme the tray heart is drawn in — mirrors the popover's
/// `Theme` (see `src/theme.ts`); the frontend syncs the choice to the backend.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TrayTheme {
    Minecraft,
    Classic,
    Arknights,
}

impl TrayTheme {
    /// Parse the localStorage theme id; unknown / default → Minecraft (the app's
    /// default theme).
    pub fn from_id(s: &str) -> Self {
        match s {
            "classic" => TrayTheme::Classic,
            "arknights" => TrayTheme::Arknights,
            _ => TrayTheme::Minecraft,
        }
    }
}

/// The heart's colours for a remaining fraction, per theme.
///
/// In every theme the *drained* pixels are dark versions of the lit hue, so the
/// fill level reads from **luminance** — a bright-vs-dark contrast that survives
/// the menu bar's vibrancy, which can wash the *hue* out entirely (a red can
/// render as a muddy blue over a blue wallpaper). Only **Classic** also ramps the
/// hue green → red with danger (its whole identity); **Minecraft** stays its
/// iconic red and **Arknights** its 理智 azure — for those, danger reads from how
/// drained the heart is plus the tray-title `%` (see `ambient`).
pub fn zone_style(remaining: f64, theme: TrayTheme) -> HeartStyle {
    match theme {
        TrayTheme::Classic => classic_style(remaining),
        TrayTheme::Minecraft => fixed_hue_style(
            (0xe0, 0x1c, 0x1c), // body — matches src/hearts.ts BODY_FULL
            (0xff, 0xf2, 0xf2), // sparkle — SPARKLE_FULL
            (0x7a, 0x0f, 0x0f), // outline
            (0x45, 0x29, 0x29), // empty body — BODY_EMPTY (the in-app drained heart)
            (0x57, 0x38, 0x38), // empty sparkle — SPARKLE_EMPTY
        ),
        TrayTheme::Arknights => fixed_hue_style(
            (0x3c, 0xc6, 0xf0), // body — 理智 azure
            (0xd6, 0xf6, 0xff), // sparkle
            (0x10, 0x5a, 0x80), // outline
            (0x15, 0x39, 0x4d), // empty body — dark navy
            (0x22, 0x55, 0x6e), // empty sparkle
        ),
    }
}

/// Classic theme: green → amber → orange → red HP ramp, dark drained pixels.
fn classic_style(remaining: f64) -> HeartStyle {
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

/// A theme whose hue is fixed (Minecraft red, Arknights azure): the lit heart is
/// always the same colour and only the fill level moves.
fn fixed_hue_style(
    body: Rgb,
    sparkle: Rgb,
    outline: Rgb,
    empty_body: Rgb,
    empty_sparkle: Rgb,
) -> HeartStyle {
    HeartStyle {
        full_body: body,
        full_sparkle: sparkle,
        full_outline: outline,
        empty_body,
        empty_sparkle,
        empty_outline: darken(outline, 0.6),
    }
}

/// The output-pixel row at/above which pixels are lit, chosen so the lit heart
/// *area* is fraction `r` of the whole. Fills from the bottom up, spending the
/// area budget row by row (partial-lighting the row it runs out on), so the
/// gauge reads true — 25% remaining lights ~a quarter of the heart, not a sliver.
/// Returns `0.0` at full (all lit) and `h` at empty (none lit).
fn area_fill_line(r: f64, h: u32) -> f64 {
    let total: f64 = ROW_WEIGHT.iter().sum::<u32>() as f64;
    let target = r.clamp(0.0, 1.0) * total; // area to light, measured from the bottom
    let mut acc = 0.0;
    let mut boundary = GRID as f64; // grid-row line (from the top); GRID = bottom edge = empty
    for row in (0..GRID as usize).rev() {
        let wgt = ROW_WEIGHT[row] as f64;
        if acc + wgt <= target {
            acc += wgt;
            boundary = row as f64; // this whole row is lit; line rises to its top edge
        } else {
            let frac = if wgt > 0.0 { (target - acc) / wgt } else { 0.0 }; // lit share of this row
            boundary = row as f64 + (1.0 - frac); // light only its bottom `frac`
            break;
        }
    }
    boundary / GRID as f64 * h as f64
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
    // Placed by heart *area* (not raw height) so the level reads honestly.
    let fill_line = area_fill_line(r, h);

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

/// Render the heart at `remaining` in `theme`'s palette.
pub fn render_rgba(remaining: f64, scale: u32, theme: TrayTheme) -> (Vec<u8>, u32, u32) {
    render_rgba_styled(remaining, scale, &zone_style(remaining, theme))
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
        let (rgba, w, h) = render_rgba(0.5, 6, TrayTheme::Classic);
        assert_eq!(w, 42);
        assert_eq!(h, 42);
        assert_eq!(rgba.len(), (42 * 42 * 4) as usize);
        // The very top-left cell (code 0) must be transparent.
        assert_eq!(rgba[3], 0);
    }

    #[test]
    fn themes_differ_when_healthy() {
        // A healthy heart is green in Classic but red in Minecraft and azure in
        // Arknights — the body pixel must differ.
        let body = |theme| {
            let (rgba, _, _) = render_rgba(0.9, 6, theme);
            // Sample a guaranteed body pixel: centre of the grid (cell 3,2 = body).
            let (w, scale) = (42u32, 6u32);
            let (x, y) = (3 * scale + 2, 2 * scale + 2);
            let i = ((y * w + x) * 4) as usize;
            (rgba[i], rgba[i + 1], rgba[i + 2])
        };
        let mc = body(TrayTheme::Minecraft);
        let classic = body(TrayTheme::Classic);
        let ak = body(TrayTheme::Arknights);
        assert!(mc != classic && classic != ak && mc != ak, "themes should differ: {mc:?} {classic:?} {ak:?}");
        assert!(classic.1 > classic.0 && classic.1 > classic.2, "classic healthy should be green-dominant");
        assert!(ak.2 > ak.0, "arknights should be blue-dominant");
    }

    #[test]
    fn from_id_maps_themes() {
        assert_eq!(TrayTheme::from_id("classic"), TrayTheme::Classic);
        assert_eq!(TrayTheme::from_id("arknights"), TrayTheme::Arknights);
        assert_eq!(TrayTheme::from_id("minecraft"), TrayTheme::Minecraft);
        assert_eq!(TrayTheme::from_id("garbage"), TrayTheme::Minecraft);
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
    fn fill_tracks_area_not_height() {
        // Lit fraction should follow the *area* budget: a 25%-remaining heart must
        // light roughly a quarter of its pixels, not the ~3% a height-based fill
        // gave (which made the tray heart look nearly empty). Use a style whose lit
        // pixels are uniquely identifiable so we can count them.
        let style = HeartStyle {
            full_body: (0, 255, 0),
            full_sparkle: (0, 255, 0),
            full_outline: (0, 255, 0),
            empty_body: (10, 10, 10),
            empty_sparkle: (10, 10, 10),
            empty_outline: (10, 10, 10),
        };
        let lit_frac = |r: f64| {
            let (rgba, w, h) = render_rgba_styled(r, 8, &style);
            let lit = (0..(w * h))
                .filter(|i| {
                    let p = (*i * 4) as usize;
                    rgba[p + 3] == 0xff && rgba[p + 1] == 255 && rgba[p] == 0
                })
                .count();
            let total = (0..(w * h)).filter(|i| rgba[(*i * 4 + 3) as usize] == 0xff).count();
            lit as f64 / total as f64
        };
        // Within ~8% of the target — chunky pixels won't be exact, but must be far
        // from the old height-based 0.03 at r=0.25.
        assert!((lit_frac(0.25) - 0.25).abs() < 0.08, "25% got {}", lit_frac(0.25));
        assert!((lit_frac(0.50) - 0.50).abs() < 0.08, "50% got {}", lit_frac(0.50));
        assert!((lit_frac(0.75) - 0.75).abs() < 0.08, "75% got {}", lit_frac(0.75));
    }

    #[test]
    fn empty_heart_still_has_silhouette() {
        // At 0% the heart must still be drawn, so the icon stays findable.
        let (rgba, w, h) = render_rgba(0.0, 6, TrayTheme::Classic);
        let opaque = (0..(w * h)).filter(|i| rgba[(*i * 4 + 3) as usize] == 0xff).count();
        assert!(opaque > 200, "drained heart should still render a visible silhouette");
    }
}
