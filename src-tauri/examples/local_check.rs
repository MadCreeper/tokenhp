//! Headless smoke test for the Local-activity data path: aggregate per-model
//! token usage from ~/.claude/projects and print the top models with cost.
//!
//!     cargo run --example local_check

use hpbar_lib::localstats;

fn main() {
    for (label, secs) in [("24h", 86_400i64), ("7d", 604_800)] {
        match localstats::fetch(secs) {
            Ok(r) => {
                println!("=== {label}  ({}) ===", r.source_label);
                for m in r.models.iter().take(6) {
                    let cost = m
                        .cost
                        .as_ref()
                        .map(|c| format!("${:.2}", c.total))
                        .unwrap_or_else(|| "—".into());
                    println!(
                        "  {:<14} in={:>8} out={:>8} cacheR={:>9} cacheW={:>8}  total={:>9}  {}",
                        m.display_name, m.input, m.output, m.cache_read, m.cache_create, m.total, cost
                    );
                }
            }
            Err(e) => println!("=== {label} ===  {e}"),
        }
    }
}
