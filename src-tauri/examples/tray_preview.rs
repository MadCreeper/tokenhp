//! Visual check for the live-HP tray icon. Renders the heart in each theme at a
//! range of remaining-quota levels, composited onto a light *and* a dark strip
//! (the two menu-bar appearances), then writes one PNG sheet.
//!
//!     cargo run --example tray_preview
//!     # writes /tmp/hpbar_tray_preview.png
//!
//! Not shipped — a dev aid, like `examples/check.rs`.

use hpbar_lib::heart_icon::{render_rgba, TrayTheme};
use image::{Rgba, RgbaImage};

const SCALE: u32 = 8; // render big; the OS downsamples to ~18px in the bar
const LEVELS: &[f64] = &[1.0, 0.85, 0.6, 0.5, 0.45, 0.25, 0.18, 0.1, 0.05, 0.0];
const THEMES: &[(&str, TrayTheme)] = &[
    ("minecraft", TrayTheme::Minecraft),
    ("classic", TrayTheme::Classic),
    ("arknights", TrayTheme::Arknights),
];

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
    let (_, w, h) = render_rgba(1.0, SCALE, TrayTheme::Classic);
    let pad = 6;
    let (tile_w, tile_h) = (w + pad * 2, h + pad * 2);
    let gap = 8;

    // For each theme: a row on a light bar, then a row on a dark bar.
    let light = Rgba([245, 245, 245, 255]);
    let dark = Rgba([38, 38, 40, 255]);

    let cols = LEVELS.len() as u32;
    let rows = THEMES.len() as u32 * 2;
    let sheet_w = cols * (tile_w + gap) + gap;
    let sheet_h = rows * (tile_h + gap) + gap;
    let mut sheet = RgbaImage::from_pixel(sheet_w, sheet_h, Rgba([20, 20, 20, 255]));

    for (ti, (_name, theme)) in THEMES.iter().enumerate() {
        for (bi, bg) in [light, dark].into_iter().enumerate() {
            let row = ti as u32 * 2 + bi as u32;
            for (ci, &lvl) in LEVELS.iter().enumerate() {
                let (rgba, w, h) = render_rgba(lvl, SCALE, *theme);
                let tile = composite(bg, &rgba, w, h, pad);
                let ox = gap + ci as u32 * (tile_w + gap);
                let oy = gap + row * (tile_h + gap);
                blit(&mut sheet, &tile, ox, oy);
            }
        }
    }

    let out = "/tmp/hpbar_tray_preview.png";
    sheet.save(out).expect("write preview png");
    println!("levels (L→R): {LEVELS:?}");
    println!("rows: each theme on a light bar then a dark bar, in order:");
    for (name, _) in THEMES {
        println!("  - {name}");
    }
    println!("wrote {out} ({sheet_w}×{sheet_h})");
}
