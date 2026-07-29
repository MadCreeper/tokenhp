//! Headless check for the Codex data path: aggregate local usage, fetch the
//! live rate-limit quota, and read the local snapshot fallback. Run with:
//!
//!     cargo run --example codex_check

use hpbar_lib::account;
use hpbar_lib::codexquota;
use hpbar_lib::codexstats;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let a = account::fetch_codex();
    println!("codex account: email={:?} plan={:?}", a.email, a.plan);
    println!("---");
    let models = codexstats::collect_local(2_592_000);
    if models.is_empty() {
        println!("local: (no Codex usage in window)");
    }
    for m in &models {
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
    println!("---");
    match codexquota::fetch().await {
        Ok(r) => print_report("live", &r),
        Err(e) => println!("live quota: {e}"),
    }
    match codexstats::fetch_quota(codexstats::SNAPSHOT_MAX_AGE_SECS) {
        Ok(r) => print_report("local fallback", &r),
        Err(e) => println!("local fallback: {e}"),
    }
}

fn print_report(tag: &str, r: &hpbar_lib::usage::UsageReport) {
    println!("{tag}: {}", r.source_label);
    for w in &r.windows {
        println!(
            "  {:<30} {:>3}% used  resets_at={:?}",
            w.title,
            (w.utilization * 100.0).round() as i64,
            w.resets_at
        );
    }
    for d in &r.details {
        println!("  · {}: {}", d.label, d.value);
    }
}
