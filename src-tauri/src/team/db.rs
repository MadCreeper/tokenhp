//! Team store backed by a self-hosted Postgres, reached over an SSH tunnel.
//!
//! No HTTP is involved: Postgres speaks its own TCP wire protocol, and we never
//! expose it publicly. The DB binds to localhost on the VPS; each client opens a
//! short-lived SSH tunnel (`ssh -N -L …`, reusing the system ssh client) and
//! talks Postgres over it with `NoTls` — the tunnel already encrypts, which also
//! keeps us off OpenSSL. The SSH key is the auth; Postgres `trust`s the
//! localhost tunnel as a limited role, so HPBar stores no DB password.
//!
//! Each member only ever writes its own rows (keyed by `member_id`), so
//! concurrent uploads from different machines never conflict — Postgres handles
//! that natively. A tunnel is opened per operation (uploads are ~30 min apart,
//! reads on demand), so there's no long-lived connection to supervise.

use super::TeamConfig;
use crate::localstats::UsageRow;
use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tokio_postgres::{Client, NoTls};

/// A member is "stale" (row fades) once their last activity is older than this
/// — scaled to the viewed range: 2h reads right for Today, but over 7d/30d the
/// question is "still contributing to this window", not "at the keyboard now".
fn stale_secs(range: &str) -> i64 {
    match range {
        "week" => 12 * 3600,
        "month" => 48 * 3600,
        _ => 2 * 3600, // "day"
    }
}
/// Arbitrary key so concurrent migrators serialize via `pg_advisory_xact_lock`.
const MIGRATE_LOCK: i64 = 0x4850_4241_52; // "HPBAR"

/// Ordered schema migrations. To evolve the schema later, append a new
/// `(version, sql)` entry — it runs once on the next connect.
const MIGRATIONS: &[(i32, &str)] = &[(1, MIGRATION_V1)];

const MIGRATION_V1: &str = "
CREATE TABLE IF NOT EXISTS members (
  member_id text PRIMARY KEY,
  display_name text NOT NULL DEFAULT '',
  host text,
  email text,
  app_version text,
  updated_at timestamptz,
  current_project text
);
CREATE TABLE IF NOT EXISTS usage_daily (
  member_id text NOT NULL REFERENCES members(member_id) ON DELETE CASCADE,
  day date NOT NULL,
  project text NOT NULL,
  model text NOT NULL,
  input bigint NOT NULL DEFAULT 0,
  output bigint NOT NULL DEFAULT 0,
  cache_read bigint NOT NULL DEFAULT 0,
  cache_create bigint NOT NULL DEFAULT 0,
  tokens bigint NOT NULL DEFAULT 0,
  cost double precision NOT NULL DEFAULT 0,
  PRIMARY KEY (member_id, day, project, model)
);
CREATE INDEX IF NOT EXISTS usage_daily_day_idx ON usage_daily(day);
CREATE TABLE IF NOT EXISTS team_meta (key text PRIMARY KEY, value text);
";

// --- report shapes (to the frontend; unchanged from the git version) --------

/// One model's usage for a member over the range (for the per-model leaderboard).
#[derive(Serialize, Clone)]
pub struct ModelUsage {
    pub model: String,
    pub display_name: String,
    pub tokens: i64,
    pub cost: f64,
}

/// One project's usage for a member over the range (for the expandable row).
#[derive(Serialize, Clone)]
pub struct ProjectUsage {
    pub project: String,
    pub tokens: i64,
    pub cost: f64,
}

#[derive(Serialize, Clone)]
pub struct MemberView {
    pub member_id: String,
    pub display_name: String,
    pub tokens: i64, // total across all models
    pub cost: f64,
    pub current_project: Option<String>,
    pub last_seen_secs: i64,
    pub is_stale: bool,
    pub is_self: bool,
    /// Per-model breakdown so the frontend can switch models without refetching.
    pub by_model: Vec<ModelUsage>,
    /// Per-project breakdown (desc by tokens) for the expandable "top projects".
    pub by_project: Vec<ProjectUsage>,
}

/// A model present in the range, for the dropdown (sorted by team-wide usage).
#[derive(Serialize, Clone)]
pub struct ModelOption {
    pub id: String,
    pub display_name: String,
    pub tokens: i64,
}

#[derive(Serialize, Clone)]
pub struct TeamReport {
    pub team_name: String,
    pub range: String,
    pub members: Vec<MemberView>,
    pub models: Vec<ModelOption>,
    pub generated_at: String,
}

#[derive(Serialize, Clone)]
pub struct TeamHandshake {
    pub ok: bool,
    pub team_name: String,
    pub member_count: usize,
    pub members: Vec<String>,
}

// --- public operations -------------------------------------------------------

/// Upsert this member's identity and replace their recent usage window.
pub async fn upload(cfg: &TeamConfig, email: Option<&str>, rows: Vec<UsageRow>) -> Result<(), String> {
    let mut conn = connect(cfg).await?;
    migrate(&mut conn.client).await?;
    write_member(&mut conn.client, cfg, email, &rows).await
}

/// Aggregate the leaderboard for `range` ("day" | "week" | "month").
pub async fn fetch(cfg: &TeamConfig, range: &str) -> Result<TeamReport, String> {
    let mut conn = connect(cfg).await?;
    migrate(&mut conn.client).await?;
    read_team(&conn.client, cfg, range).await
}

/// The handshake: connect, migrate, write our member, read the roster back.
pub async fn handshake(
    cfg: &TeamConfig,
    email: Option<&str>,
    rows: Vec<UsageRow>,
) -> Result<TeamHandshake, String> {
    let mut conn = connect(cfg).await?;
    migrate(&mut conn.client).await?;
    write_member(&mut conn.client, cfg, email, &rows).await?;
    let report = read_team(&conn.client, cfg, "day").await?;
    let members = report.members.iter().map(|m| m.display_name.clone()).collect();
    Ok(TeamHandshake {
        ok: true,
        team_name: report.team_name,
        member_count: report.members.len(),
        members,
    })
}

// --- connection + tunnel -----------------------------------------------------

/// A live Postgres client plus the tunnel that backs it. Dropping it closes the
/// client and tears down the ssh child (field order matters: client first).
struct Conn {
    client: Client,
    _tunnel: Option<Tunnel>,
}

async fn connect(cfg: &TeamConfig) -> Result<Conn, String> {
    // `HPBAR_DB_DIRECT=host:port` skips the tunnel (for local testing).
    let (host, port, tunnel) = match std::env::var("HPBAR_DB_DIRECT").ok() {
        Some(hp) => {
            let (h, p) = hp.split_once(':').ok_or("HPBAR_DB_DIRECT must be host:port")?;
            let port: u16 = p.parse().map_err(|_| "bad HPBAR_DB_DIRECT port")?;
            (h.to_string(), port, None)
        }
        None => {
            if cfg.ssh_host.trim().is_empty() {
                return Err("SSH host is not configured.".to_string());
            }
            let t = open_tunnel(cfg).await?;
            ("127.0.0.1".to_string(), t.port, Some(t))
        }
    };

    let conf = format!(
        "host={host} port={port} dbname={} user={} application_name=hpbar connect_timeout=10",
        cfg.db_name, cfg.db_user
    );
    let (client, connection) = tokio_postgres::connect(&conf, NoTls)
        .await
        .map_err(|e| format!("Postgres connect failed: {e}"))?;
    // The connection drives the protocol; it ends when `client` is dropped.
    tokio::spawn(async move {
        let _ = connection.await;
    });
    Ok(Conn {
        client,
        _tunnel: tunnel,
    })
}

struct Tunnel {
    child: Child,
    port: u16,
    /// Temp SSH_ASKPASS helper to remove when the tunnel closes (password auth).
    askpass: Option<PathBuf>,
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        cleanup_askpass(&self.askpass);
    }
}

async fn open_tunnel(cfg: &TeamConfig) -> Result<Tunnel, String> {
    let port = free_local_port()?;
    let forward = format!("127.0.0.1:{port}:{}:{}", cfg.db_host, cfg.db_port);
    let target = format!("{}@{}", cfg.ssh_user, cfg.ssh_host);
    let ssh_port = cfg.ssh_port.to_string();

    let mut cmd = Command::new("ssh");
    cmd.args([
        "-N",
        "-o",
        "ExitOnForwardFailure=yes",
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "ServerAliveInterval=15",
        "-p",
        &ssh_port,
        "-L",
        &forward,
        &target,
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::piped());

    // Optional password auth, fed via SSH_ASKPASS so we never block on a prompt.
    // Without a password we stay key/agent-only (BatchMode) so a missing
    // credential errors fast instead of hanging.
    let askpass = setup_auth(&mut cmd, cfg)?;

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            cleanup_askpass(&askpass);
            return Err(format!("could not run ssh ({e}); is it installed and on PATH?"));
        }
    };

    // Wait (≤10s) for the forwarded port to accept connections.
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return Ok(Tunnel {
                child,
                port,
                askpass,
            });
        }
        if let Ok(Some(status)) = child.try_wait() {
            let msg = drain_stderr(&mut child);
            cleanup_askpass(&askpass);
            return Err(format!("ssh tunnel exited ({status}): {msg}"));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let _ = child.kill();
    let _ = child.wait();
    let msg = drain_stderr(&mut child);
    cleanup_askpass(&askpass);
    Err(format!("ssh tunnel did not come up within 10s: {msg}"))
}

/// Configure the ssh auth method on `cmd`. Returns the temp askpass helper to
/// clean up (Some only when a password is in use, unix only).
#[cfg(unix)]
fn setup_auth(cmd: &mut Command, cfg: &TeamConfig) -> Result<Option<PathBuf>, String> {
    if cfg.ssh_password.trim().is_empty() {
        cmd.arg("-o").arg("BatchMode=yes");
        return Ok(None);
    }
    let path = write_askpass_helper()?;
    cmd.env("HPBAR_TUNNEL_PW", &cfg.ssh_password)
        .env("SSH_ASKPASS", &path)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("DISPLAY", ":0")
        .arg("-o")
        .arg("BatchMode=no")
        .arg("-o")
        .arg("NumberOfPasswordPrompts=1");
    Ok(Some(path))
}

#[cfg(not(unix))]
fn setup_auth(cmd: &mut Command, _cfg: &TeamConfig) -> Result<Option<PathBuf>, String> {
    // The SSH_ASKPASS helper is a /bin/sh script, so password auth is unix-only;
    // elsewhere fall back to key/agent auth.
    cmd.arg("-o").arg("BatchMode=yes");
    Ok(None)
}

fn cleanup_askpass(askpass: &Option<PathBuf>) {
    if let Some(p) = askpass {
        let _ = std::fs::remove_file(p);
    }
}

/// Write a one-shot SSH_ASKPASS helper that echoes `$HPBAR_TUNNEL_PW`.
#[cfg(unix)]
fn write_askpass_helper() -> Result<PathBuf, String> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut path = std::env::temp_dir();
    path.push(format!("hpbar-askpass-{}-{}.sh", std::process::id(), n));
    let mut f = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    f.write_all(b"#!/bin/sh\nprintf '%s\\n' \"$HPBAR_TUNNEL_PW\"\n")
        .map_err(|e| e.to_string())?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    Ok(path)
}

fn free_local_port() -> Result<u16, String> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| e.to_string())
}

/// Best-effort read of a *terminated* ssh child's stderr for the error message.
fn drain_stderr(child: &mut Child) -> String {
    use std::io::Read;
    let mut out = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut out);
    }
    out.trim().to_string()
}

// --- migrations + queries ----------------------------------------------------

async fn migrate(client: &mut Client) -> Result<(), String> {
    client
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS schema_migrations (version int PRIMARY KEY, applied_at timestamptz DEFAULT now())",
        )
        .await
        .map_err(|e| format!("migrate (init) failed: {e}"))?;

    let tx = client
        .transaction()
        .await
        .map_err(|e| e.to_string())?;
    // Serialize concurrent migrators; auto-released at commit.
    tx.execute("SELECT pg_advisory_xact_lock($1)", &[&MIGRATE_LOCK])
        .await
        .map_err(|e| e.to_string())?;
    let current: i32 = tx
        .query_one("SELECT COALESCE(MAX(version), 0)::int FROM schema_migrations", &[])
        .await
        .map_err(|e| e.to_string())?
        .get(0);
    for (version, sql) in MIGRATIONS {
        if *version > current {
            tx.batch_execute(sql)
                .await
                .map_err(|e| format!("migration {version} failed: {e}"))?;
            tx.execute("INSERT INTO schema_migrations(version) VALUES($1)", &[version])
                .await
                .map_err(|e| e.to_string())?;
        }
    }
    tx.commit().await.map_err(|e| e.to_string())
}

async fn write_member(
    client: &mut Client,
    cfg: &TeamConfig,
    email: Option<&str>,
    rows: &[UsageRow],
) -> Result<(), String> {
    // Most recent activity → current project + staleness anchor.
    let newest = rows.iter().max_by_key(|r| r.last_active);
    let updated_ts = newest.map(|r| r.last_active).unwrap_or(0);
    let updated_at: Option<DateTime<Utc>> = if updated_ts > 0 {
        DateTime::from_timestamp(updated_ts, 0)
    } else {
        None
    };
    let current_project = if cfg.share_project {
        newest.map(|r| project_label(&r.project, cfg))
    } else {
        None
    };
    let host = super::hostname();
    let app_version = env!("CARGO_PKG_VERSION");
    let team_name = if cfg.team_name.trim().is_empty() {
        "Team".to_string()
    } else {
        cfg.team_name.clone()
    };
    let start_date =
        (Utc::now() - chrono::Duration::days(cfg.backfill_days.max(0))).date_naive();
    let email_owned = email.map(str::to_string);

    let tx = client.transaction().await.map_err(|e| e.to_string())?;

    tx.execute(
        "INSERT INTO members(member_id, display_name, host, email, app_version, updated_at, current_project)
         VALUES($1,$2,$3,$4,$5,$6,$7)
         ON CONFLICT (member_id) DO UPDATE SET
           display_name=excluded.display_name, host=excluded.host, email=excluded.email,
           app_version=excluded.app_version, updated_at=excluded.updated_at,
           current_project=excluded.current_project",
        &[
            &cfg.member_id,
            &cfg.display_name,
            &host,
            &email_owned,
            &app_version,
            &updated_at,
            &current_project,
        ],
    )
    .await
    .map_err(|e| format!("upsert member failed: {e}"))?;

    // Replace this member's recent window wholesale (idempotent; only our rows).
    tx.execute(
        "DELETE FROM usage_daily WHERE member_id=$1 AND day >= $2",
        &[&cfg.member_id, &start_date],
    )
    .await
    .map_err(|e| e.to_string())?;

    for r in rows {
        let Ok(day) = NaiveDate::parse_from_str(&r.day, "%Y-%m-%d") else {
            continue;
        };
        let project = project_label(&r.project, cfg);
        let (input, output, cache_read, cache_create, tokens) = if cfg.share_tokens {
            (r.input, r.output, r.cache_read, r.cache_create, r.tokens)
        } else {
            (0, 0, 0, 0, 0)
        };
        let cost = if cfg.share_cost { r.cost } else { 0.0 };
        tx.execute(
            "INSERT INTO usage_daily(member_id, day, project, model, input, output, cache_read, cache_create, tokens, cost)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
             ON CONFLICT (member_id, day, project, model) DO UPDATE SET
               input=excluded.input, output=excluded.output, cache_read=excluded.cache_read,
               cache_create=excluded.cache_create, tokens=excluded.tokens, cost=excluded.cost",
            &[
                &cfg.member_id, &day, &project, &r.model,
                &input, &output, &cache_read, &cache_create, &tokens, &cost,
            ],
        )
        .await
        .map_err(|e| format!("insert usage failed: {e}"))?;
    }

    tx.execute(
        "INSERT INTO team_meta(key, value) VALUES('team_name', $1) ON CONFLICT (key) DO NOTHING",
        &[&team_name],
    )
    .await
    .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())
}

async fn read_team(client: &Client, cfg: &TeamConfig, range: &str) -> Result<TeamReport, String> {
    let day_filter = match range {
        "week" => "day >= current_date - 6",
        "month" => "day >= date_trunc('month', current_date)::date",
        _ => "day = current_date",
    };

    // Per-(member, model) totals for the range — the frontend slices this by
    // model client-side so the dropdown switches instantly.
    let usage_sql = format!(
        "SELECT member_id, model, SUM(tokens)::bigint AS tokens,
                COALESCE(SUM(cost), 0)::double precision AS cost
         FROM usage_daily WHERE {day_filter} GROUP BY member_id, model"
    );
    let usage_rows = client.query(&usage_sql, &[]).await.map_err(|e| e.to_string())?;

    let mut by_member: std::collections::HashMap<String, Vec<ModelUsage>> =
        std::collections::HashMap::new();
    let mut model_totals: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for row in &usage_rows {
        let member_id: String = row.get("member_id");
        let model: String = row.get("model");
        let tokens: i64 = row.get("tokens");
        let cost: f64 = row.get("cost");
        *model_totals.entry(model.clone()).or_default() += tokens;
        by_member.entry(member_id).or_default().push(ModelUsage {
            display_name: crate::localstats::display_name(&model),
            model,
            tokens,
            cost,
        });
    }

    // Per-(member, project) totals for the range (the expandable "top projects").
    let project_sql = format!(
        "SELECT member_id, project, SUM(tokens)::bigint AS tokens,
                COALESCE(SUM(cost), 0)::double precision AS cost
         FROM usage_daily WHERE {day_filter} GROUP BY member_id, project"
    );
    let project_rows = client.query(&project_sql, &[]).await.map_err(|e| e.to_string())?;
    let mut projects_by_member: std::collections::HashMap<String, Vec<ProjectUsage>> =
        std::collections::HashMap::new();
    for row in &project_rows {
        let member_id: String = row.get("member_id");
        projects_by_member.entry(member_id).or_default().push(ProjectUsage {
            project: row.get("project"),
            tokens: row.get("tokens"),
            cost: row.get("cost"),
        });
    }

    // Member identity/metadata (everyone, even with no usage in the range).
    let meta_rows = client
        .query(
            "SELECT member_id, display_name, current_project,
                    EXTRACT(EPOCH FROM updated_at)::bigint AS updated_ts
             FROM members",
            &[],
        )
        .await
        .map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().timestamp();

    let mut members: Vec<MemberView> = meta_rows
        .iter()
        .map(|row| {
            let member_id: String = row.get("member_id");
            let raw_name: String = row.get("display_name");
            let display_name = if raw_name.is_empty() {
                member_id.clone()
            } else {
                raw_name
            };
            let updated_ts: Option<i64> = row.get("updated_ts");
            let updated_ts = updated_ts.unwrap_or(0);
            let last_seen_secs = if updated_ts > 0 {
                (now - updated_ts).max(0)
            } else {
                i64::MAX
            };
            let mut by_model = by_member.remove(&member_id).unwrap_or_default();
            by_model.sort_by(|a, b| b.tokens.cmp(&a.tokens));
            let mut by_project = projects_by_member.remove(&member_id).unwrap_or_default();
            by_project.sort_by(|a, b| b.tokens.cmp(&a.tokens));
            let tokens = by_model.iter().map(|m| m.tokens).sum();
            let cost = by_model.iter().map(|m| m.cost).sum();
            MemberView {
                is_self: member_id == cfg.member_id,
                member_id,
                display_name,
                tokens,
                cost,
                current_project: row.get("current_project"),
                last_seen_secs,
                is_stale: updated_ts == 0 || last_seen_secs > stale_secs(range),
                by_model,
                by_project,
            }
        })
        .collect();
    members.sort_by(|a, b| b.tokens.cmp(&a.tokens).then(a.display_name.cmp(&b.display_name)));

    let mut models: Vec<ModelOption> = model_totals
        .into_iter()
        .map(|(id, tokens)| ModelOption {
            display_name: crate::localstats::display_name(&id),
            id,
            tokens,
        })
        .collect();
    models.sort_by(|a, b| b.tokens.cmp(&a.tokens).then(a.id.cmp(&b.id)));

    let team_name = client
        .query_opt("SELECT value FROM team_meta WHERE key = 'team_name'", &[])
        .await
        .map_err(|e| e.to_string())?
        .and_then(|r| r.get::<_, Option<String>>(0))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| {
            if cfg.team_name.trim().is_empty() {
                "Team".to_string()
            } else {
                cfg.team_name.clone()
            }
        });

    Ok(TeamReport {
        team_name,
        range: range.to_string(),
        members,
        models,
        generated_at: chrono::Utc::now().to_rfc3339(),
    })
}

/// Apply the project-name privacy toggle.
fn project_label(project: &str, cfg: &TeamConfig) -> String {
    if cfg.share_project {
        project.to_string()
    } else {
        "(hidden)".to_string()
    }
}
