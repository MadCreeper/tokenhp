//! Headless check for the Codex data path: aggregate local usage and read the
//! latest rate-limit quota from ~/.codex/sessions. Run with:
//!
//!     cargo run --example codex_check

use hpbar_lib::codexstats;

fn main() {
    match codexstats::fetch_local(2_592_000) {
        Ok(r) => {
            println!("{}", r.source_label);
            for m in &r.models {
                let cost = m
                    .cost
                    .as_ref()
                    .map(|c| format!(" ${:.4}", c.total))
                    .unwrap_or_default();
                println!(
                    "  {:<14} in={} out={} cacheR={}{}",
                    m.display_name, m.input, m.output, m.cache_read, cost
                );
            }
        }
        Err(e) => println!("local: {e}"),
    }
    println!("---");
    match codexstats::fetch_quota() {
        Ok(r) => {
            println!("{}", r.source_label);
            for w in &r.windows {
                println!(
                    "  {:<8} {:>3}% used  resets_at={:?}",
                    w.title,
                    (w.utilization * 100.0).round() as i64,
                    w.resets_at
                );
            }
        }
        Err(e) => println!("quota: {e}"),
    }
}
