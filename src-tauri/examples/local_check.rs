//! Headless smoke test for the API (Local) data path: aggregate per-tool +
//! pooled token usage across every local tool and print the breakdown.
//!
//!     cargo run --example local_check

use hpbar_lib::tools;

fn main() {
    for (label, secs) in [("24h", 86_400i64), ("7d", 604_800)] {
        match tools::fetch_local(secs) {
            Ok(r) => {
                println!("=== {label}  ({}) ===", r.source_label);
                for app in &r.apps {
                    let cost = app
                        .cost
                        .map(|c| format!("${:.2}", c))
                        .unwrap_or_else(|| "—".into());
                    println!("  [{}] {} ({})  total={}  {}", app.id, app.display_name, app.kind, app.total, cost);
                    for m in app.models.iter().take(6) {
                        println!(
                            "      {:<16} in={:>8} out={:>8} cacheR={:>9} cacheW={:>8}  total={:>9}",
                            m.display_name, m.input, m.output, m.cache_read, m.cache_create, m.total
                        );
                    }
                }
                println!("  -- combined (pooled by model) --");
                for m in r.combined.iter().take(6) {
                    println!("      {:<16} total={:>9}", m.display_name, m.total);
                }
            }
            Err(e) => println!("=== {label} ===  {e}"),
        }
    }
}
