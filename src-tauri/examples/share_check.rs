//! Diagnostic: load the real recorded device-share series and print what the
//! estimator produces per (provider, window) — machine/others split, Q, and the
//! confidence (which gates the UI). No network, no Tauri.
//!
//!     cargo run --example share_check

use hpbar_lib::share::{estimate, window_secs_for_title, ShareSample};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let path = dirs::config_dir()
        .expect("config dir")
        .join("com.madcreeper.hpbar")
        .join("share_history.json");
    let text = std::fs::read_to_string(&path).expect("read share_history.json");
    let hist: HashMap<String, Vec<ShareSample>> = serde_json::from_str(&text).expect("parse");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    println!("now={now}  (gate: show split when confidence ≥ 0.35)\n");
    let mut keys: Vec<_> = hist.keys().cloned().collect();
    keys.sort();
    for k in keys {
        let samples = &hist[&k];
        let title = k.split('/').nth(1).unwrap_or("");
        let cycle = window_secs_for_title(title);
        let live_u = samples.last().map(|s| s.u).unwrap_or(0.0);
        let age = samples.last().map(|s| now - s.ts).unwrap_or(-1);
        match estimate(samples, now, live_u, cycle) {
            Some(r) => println!(
                "{k:18} U={:.0}%  machine={:.0}%  others={:.0}%  Q=${:.0}  conf={:.2}  {}  ({} samples, last {}s ago)",
                live_u * 100.0,
                r.this_machine * 100.0,
                r.others * 100.0,
                r.q,
                r.confidence,
                if r.confidence >= 0.35 { "SHOWN" } else { "hidden" },
                samples.len(),
                age,
            ),
            None => println!("{k:18} no fit ({} samples)", samples.len()),
        }
    }
}
