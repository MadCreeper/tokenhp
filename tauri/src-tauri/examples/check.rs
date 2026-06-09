//! Headless smoke test for the data path: read the Claude Code credential from
//! local storage and fetch live quota from the OAuth usage endpoint, printing
//! the resulting hearts-bar windows. Run with:
//!
//!     cargo run --example check
//!
//! On macOS this triggers one Keychain prompt (the example binary isn't on the
//! item's ACL) — approve it. On Linux/Windows it reads ~/.claude/.credentials.json
//! with no prompt.

use hpbar_lib::credentials::CredentialCache;
use hpbar_lib::usage;

#[tokio::main]
async fn main() {
    let cache = CredentialCache::new();
    match usage::fetch(&cache).await {
        Ok(report) => {
            println!("source: {}", report.source_label);
            for w in &report.windows {
                let trailing = w
                    .trailing
                    .clone()
                    .unwrap_or_else(|| format!("{}%", (w.remaining * 100.0).round() as i64));
                let hearts = (w.remaining * 10.0).round() as i64;
                println!(
                    "  {:<12} {:>5}  [{}{}]  resets_at={:?}",
                    w.title,
                    trailing,
                    "♥".repeat(hearts.max(0) as usize),
                    "·".repeat((10 - hearts).max(0) as usize),
                    w.resets_at,
                );
            }
            println!("\nOK — data path works.");
        }
        Err(e) => {
            eprintln!("FETCH FAILED: {e}");
            std::process::exit(1);
        }
    }
}
