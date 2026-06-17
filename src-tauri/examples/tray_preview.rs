//! Visual check for the live-HP tray icon. Renders the heart at a range of
//! remaining-quota levels and composites each onto a light *and* a dark strip
//! (the two menu-bar appearances) plus a checkerboard, then writes one PNG sheet.
//!
//!     cargo run --example tray_preview
//!     # writes /tmp/hpbar_tray_preview.png
//!
//! Not shipped — a dev aid, like `examples/check.rs`.

use hpbar_lib::heart_icon::{render_rgba, render_rgba_styled, zone_style};
use image::{Rgba, RgbaImage};

const SCALE: u32 = 8; // render big; the OS downsamples to ~18px in the bar
const LEVELS: &[f64] = &[1.0, 0.85, 0.6, 0.5, 0.45, 0.25, 0.18, 0.1, 0.05, 0.0];

fn composite(bg: Rgba<u8>, rgba: &[u8], w: u32, h: u32, pad: u32) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(w + pad * 2, h + pad * 2, bg);
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let a = rgba[i + 3] as f32 / 255.0;
            if a == 0.0 {
                continue;
            }
            let px = img.get_pixel_mut(x + pad, y + pad);
            for c in 0..3 {
                px[c] = (rgba[i + c] as f32 * a + px[c] as f32 * (1.0 - a)) as u8;
            }
        }
    }
    img
}

fn blit(sheet: &mut RgbaImage, tile: &RgbaImage, ox: u32, oy: u32) {
    for y in 0..tile.height() {
        for x in 0..tile.width() {
            sheet.put_pixel(ox + x, oy + y, *tile.get_pixel(x, y));
        }
    }
}

fn main() {
    let (_, w, h) = render_rgba(1.0, SCALE);
    let pad = 6;
    let tile_w = w + pad * 2;
    let tile_h = h + pad * 2;
    let gap = 8;

    // Backgrounds: light bar, dark bar, checkerboard-ish mid grey. Plus a second
    // block using the all-red "amount of red remaining" style for comparison.
    let light = Rgba([245, 245, 245, 255]);
    let dark = Rgba([38, 38, 40, 255]);
    let mid = Rgba([120, 120, 120, 255]);
    let bgs = [("light", light), ("dark", dark), ("mid", mid)];

    let cols = LEVELS.len() as u32;
    // rows: 3 backgrounds for the zone-ramp style, then 3 for an all-red variant.
    let rows = bgs.len() as u32 * 2;
    let sheet_w = cols * (tile_w + gap) + gap;
    let sheet_h = rows * (tile_h + gap) + gap;
    let mut sheet = RgbaImage::from_pixel(sheet_w, sheet_h, Rgba([20, 20, 20, 255]));

    // Top block: the shipping green→red health ramp.
    for (ri, (_name, bg)) in bgs.iter().enumerate() {
        for (ci, &lvl) in LEVELS.iter().enumerate() {
            let (rgba, w, h) = render_rgba(lvl, SCALE);
            let tile = composite(*bg, &rgba, w, h, pad);
            let ox = gap + ci as u32 * (tile_w + gap);
            let oy = gap + ri as u32 * (tile_h + gap);
            blit(&mut sheet, &tile, ox, oy);
        }
    }

    // Bottom block: the previous neutral-grey-empty styling, for A/B against the
    // new hue-preserving drained pixels above.
    let grey_empty = |r: f64| {
        let mut s = zone_style(r);
        s.empty_body = (0x7c, 0x7c, 0x7c);
        s.empty_sparkle = (0x9a, 0x9a, 0x9a);
        s.empty_outline = (0x5a, 0x5a, 0x5a);
        s
    };
    for (ri, (_name, bg)) in bgs.iter().enumerate() {
        for (ci, &lvl) in LEVELS.iter().enumerate() {
            let (rgba, w, h) = render_rgba_styled(lvl, SCALE, &grey_empty(lvl));
            let tile = composite(*bg, &rgba, w, h, pad);
            let ox = gap + ci as u32 * (tile_w + gap);
            let oy = gap + (bgs.len() as u32 + ri as u32) * (tile_h + gap);
            blit(&mut sheet, &tile, ox, oy);
        }
    }

    let out = "/tmp/hpbar_tray_preview.png";
    sheet.save(out).expect("write preview png");
    println!("levels (L→R): {LEVELS:?}");
    println!("top 3 rows = NEW hue-preserving drained pixels (light/dark/mid bars)");
    println!("bottom 3 rows = OLD neutral-grey drained pixels (light/dark/mid bars)");
    println!("wrote {out} ({sheet_w}×{sheet_h})");
}
