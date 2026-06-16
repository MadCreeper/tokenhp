//! Headless smoke test for the team **DB** path (no GUI).
//!
//! Against a local Postgres with no tunnel (recommended for dev) — start one with
//!   docker run --rm -p 5432:5432 -e POSTGRES_HOST_AUTH_METHOD=trust postgres:16
//! then:
//!   HPBAR_DB_DIRECT=127.0.0.1:5432 cargo run --example team_check
//!
//! Simulate a second teammate:
//!   HPBAR_DB_DIRECT=127.0.0.1:5432 HPBAR_MEMBER=bob cargo run --example team_check
//!
//! Against a real VPS over SSH:
//!   HPBAR_SSH_HOST=vps HPBAR_SSH_USER=me cargo run --example team_check
//!
//! Env knobs: HPBAR_DB_DIRECT, HPBAR_SSH_HOST, HPBAR_SSH_USER, HPBAR_DB_NAME
//! (default "postgres"), HPBAR_DB_USER (default "postgres"), HPBAR_MEMBER.

use hpbar_lib::account;
use hpbar_lib::team::{self, TeamConfig};

#[tokio::main]
async fn main() {
    let email = account::read_email();
    let env = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());

    let mut cfg = TeamConfig {
        enabled: true,
        ssh_host: env("HPBAR_SSH_HOST").unwrap_or_default(),
        ssh_user: env("HPBAR_SSH_USER").unwrap_or_default(),
        ssh_password: env("HPBAR_SSH_PASSWORD").unwrap_or_default(),
        // Local test Postgres defaults (the `postgres` image's superuser/db).
        db_name: env("HPBAR_DB_NAME").unwrap_or_else(|| "postgres".into()),
        db_user: env("HPBAR_DB_USER").unwrap_or_else(|| "postgres".into()),
        member_id: env("HPBAR_MEMBER").unwrap_or_default(),
        ..Default::default()
    };
    cfg.normalize(email.as_deref());
    println!("member_id = {}  display = {}", cfg.member_id, cfg.display_name);

    if std::env::var("HPBAR_DB_DIRECT").is_err() && cfg.ssh_host.is_empty() {
        println!("\nset HPBAR_DB_DIRECT=host:port (local PG) or HPBAR_SSH_HOST=… (tunnel) to run.");
        return;
    }

    // Handshake (connect + migrate + write our member + read roster).
    match team::test_connection(&cfg, email.as_deref()).await {
        Ok(h) => println!(
            "handshake ok: team={} members={} {:?}",
            h.team_name, h.member_count, h.members
        ),
        Err(e) => {
            println!("handshake failed: {e}");
            return;
        }
    }

    // Leaderboards straight from the DB (no on-disk config needed).
    for range in ["day", "week", "month"] {
        match team::db::fetch(&cfg, range).await {
            Ok(r) => {
                println!("\n=== {} ({}) ===", range, r.team_name);
                for m in &r.members {
                    println!(
                        "  {:<22} {:>11} tok  ${:<8.2} proj={:?} seen={}s stale={}",
                        m.display_name, m.tokens, m.cost, m.current_project, m.last_seen_secs, m.is_stale
                    );
                }
            }
            Err(e) => println!("=== {range} === {e}"),
        }
    }
}
