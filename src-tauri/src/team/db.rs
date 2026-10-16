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
use std::collections::HashMap;
use std::process::Child;
use std::time::Duration;
use tokio_postgres::{types::ToSql, Client, NoTls, Transaction};

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
const MIGRATE_LOCK: i64 = 0x0048_5042_4152; // "HPBAR"
const CONNECT_TIMEOUT: Duration = Duration::from_secs(12);
const SESSION_SETUP_TIMEOUT: Duration = Duration::from_secs(5);
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(60);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(75);
const INSERT_CHUNK_SIZE: usize = 128;

/// Ordered schema migrations. To evolve the schema later, append a new
/// `(version, sql)` entry — it runs once on the next connect.
const MIGRATIONS: &[(i32, &str)] = &[
    (1, MIGRATION_V1),
    (2, MIGRATION_V2),
    (3, MIGRATION_V3),
    (4, MIGRATION_V4),
];

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

const MIGRATION_V2: &str = "
ALTER TABLE members ADD COLUMN IF NOT EXISTS identity_version int NOT NULL DEFAULT 1;
ALTER TABLE usage_daily ADD COLUMN IF NOT EXISTS provider text NOT NULL DEFAULT 'claude';
ALTER TABLE usage_daily ADD COLUMN IF NOT EXISTS account_key text NOT NULL DEFAULT 'unknown';
ALTER TABLE usage_daily ADD COLUMN IF NOT EXISTS billing_key text NOT NULL DEFAULT 'unknown';
ALTER TABLE usage_daily ADD COLUMN IF NOT EXISTS account_label text NOT NULL DEFAULT 'Unknown account';
ALTER TABLE usage_daily ADD COLUMN IF NOT EXISTS attribution_status text NOT NULL DEFAULT 'unknown';
ALTER TABLE usage_daily ADD COLUMN IF NOT EXISTS unattributed bigint NOT NULL DEFAULT 0;
ALTER TABLE usage_daily DROP CONSTRAINT IF EXISTS usage_daily_pkey;
ALTER TABLE usage_daily
  ADD PRIMARY KEY (member_id, day, provider, account_key, project, model);
CREATE INDEX IF NOT EXISTS usage_daily_account_idx
  ON usage_daily(provider, account_key, day);
";

const MIGRATION_V3: &str = "
ALTER TABLE members ADD COLUMN IF NOT EXISTS legacy_member_id text;
CREATE INDEX IF NOT EXISTS members_legacy_member_idx
  ON members(legacy_member_id) WHERE legacy_member_id IS NOT NULL;

-- Keep the original table writable by v1 clients. Account-aware v2 rows move
-- into their own table because the two clients require incompatible conflict
-- keys: v1 is per project/model, v2 is per provider/account/project/model.
CREATE TABLE IF NOT EXISTS usage_daily_v2 (
  member_id text NOT NULL REFERENCES members(member_id) ON DELETE CASCADE,
  day date NOT NULL,
  provider text NOT NULL,
  account_key text NOT NULL,
  billing_key text NOT NULL,
  account_label text NOT NULL,
  attribution_status text NOT NULL,
  project text NOT NULL,
  model text NOT NULL,
  input bigint NOT NULL DEFAULT 0,
  output bigint NOT NULL DEFAULT 0,
  cache_read bigint NOT NULL DEFAULT 0,
  cache_create bigint NOT NULL DEFAULT 0,
  unattributed bigint NOT NULL DEFAULT 0,
  tokens bigint NOT NULL DEFAULT 0,
  cost double precision NOT NULL DEFAULT 0,
  PRIMARY KEY (member_id, day, provider, account_key, project, model)
);
CREATE INDEX IF NOT EXISTS usage_daily_v2_day_idx ON usage_daily_v2(day);
CREATE INDEX IF NOT EXISTS usage_daily_v2_account_idx
  ON usage_daily_v2(provider, account_key, day);

INSERT INTO usage_daily_v2(
  member_id, day, provider, account_key, billing_key, account_label,
  attribution_status, project, model, input, output, cache_read,
  cache_create, unattributed, tokens, cost
)
SELECT
  u.member_id, u.day, u.provider, u.account_key, u.billing_key,
  u.account_label, u.attribution_status, u.project, u.model, u.input,
  u.output, u.cache_read, u.cache_create, u.unattributed, u.tokens, u.cost
FROM usage_daily u
JOIN members m ON m.member_id=u.member_id
WHERE m.identity_version >= 2
ON CONFLICT (member_id, day, provider, account_key, project, model)
DO UPDATE SET
  billing_key=excluded.billing_key,
  account_label=excluded.account_label,
  attribution_status=excluded.attribution_status,
  input=excluded.input,
  output=excluded.output,
  cache_read=excluded.cache_read,
  cache_create=excluded.cache_create,
  unattributed=excluded.unattributed,
  tokens=excluded.tokens,
  cost=excluded.cost;

DELETE FROM usage_daily u
USING members m
WHERE m.member_id=u.member_id AND m.identity_version >= 2;
ALTER TABLE usage_daily DROP CONSTRAINT IF EXISTS usage_daily_pkey;
ALTER TABLE usage_daily
  ADD PRIMARY KEY (member_id, day, project, model);

-- A collapsed compatibility mirror lets an old app continue to see v2 members.
-- New clients ignore these mirror rows and read the precise v2 table instead.
INSERT INTO usage_daily(
  member_id, day, project, model, input, output, cache_read, cache_create,
  unattributed, tokens, cost
)
SELECT
  member_id, day, project, model, SUM(input), SUM(output), SUM(cache_read),
  SUM(cache_create), SUM(unattributed), SUM(tokens), SUM(cost)
FROM usage_daily_v2
GROUP BY member_id, day, project, model
ON CONFLICT (member_id, day, project, model)
DO UPDATE SET
  input=excluded.input,
  output=excluded.output,
  cache_read=excluded.cache_read,
  cache_create=excluded.cache_create,
  unattributed=excluded.unattributed,
  tokens=excluded.tokens,
  cost=excluded.cost;
";

// Older clients encoded missing prices and withheld costs as the same zero.
// Preserve explicit privacy choices on new uploads; old shared token counts
// can still support a clearly marked, read-time API-rate estimate.
const MIGRATION_V4: &str = "
ALTER TABLE usage_daily ADD COLUMN IF NOT EXISTS cost_shared boolean NOT NULL DEFAULT true;
ALTER TABLE usage_daily_v2 ADD COLUMN IF NOT EXISTS cost_shared boolean NOT NULL DEFAULT true;
";

const USAGE_CTE: &str = "
WITH readable_usage AS (
  SELECT
    member_id, day, provider, account_key, billing_key, account_label,
    attribution_status, project, model, input, output, cache_read,
    cache_create, unattributed, tokens, cost, cost_shared
  FROM usage_daily_v2
  UNION ALL
  SELECT
    u.member_id, u.day, u.provider, u.account_key, u.billing_key,
    u.account_label, u.attribution_status, u.project, u.model, u.input,
    u.output, u.cache_read, u.cache_create, u.unattributed, u.tokens, u.cost, u.cost_shared
  FROM usage_daily u
  JOIN members legacy_member ON legacy_member.member_id=u.member_id
  WHERE legacy_member.identity_version < 2
)
";

/// Keep v1 members visible during a rolling upgrade, except once a v2 row
/// explicitly identifies that v1 id as its predecessor. This avoids showing a
/// user's backfilled v2 usage alongside the same legacy history while still
/// allowing teammates on the old app to remain in the roster.
const VISIBLE_MEMBER: &str = "
  NOT (
    m.identity_version < 2
    AND EXISTS (
      SELECT 1 FROM members successor
      WHERE successor.identity_version >= 2
        AND successor.legacy_member_id = m.member_id
    )
  )
";

// --- report shapes -----------------------------------------------------------

/// One model's usage for a member over the range (for the per-model leaderboard).
#[derive(Serialize, Clone)]
pub struct ModelUsage {
    pub cost_complete: bool,
    pub cost_estimated: bool,
    pub model: String,
    pub display_name: String,
    pub tokens: i64,
    pub cost: f64,
}

/// One project's usage for a member over the range (for the expandable row).
#[derive(Serialize, Clone)]
pub struct ProjectUsage {
    pub by_model: Vec<ModelUsage>,
    pub project: String,
    pub tokens: i64,
    pub cost: f64,
}

/// One login account's usage inside a member row. This is the data behind the
/// optional "Usage breakdown" drill-down and bill split.
#[derive(Serialize, Clone)]
pub struct AccountUsage {
    pub provider: String,
    pub account_key: String,
    pub billing_key: String,
    pub account_label: String,
    pub attribution_status: String,
    pub tokens: i64,
    pub cost: f64,
    pub by_model: Vec<ModelUsage>,
}

#[derive(Serialize, Clone)]
pub struct MemberView {
    pub member_id: String,
    pub identity_version: i32,
    pub is_legacy: bool,
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
    /// Per-account → per-model billing detail.
    pub by_account: Vec<AccountUsage>,
}

/// A model present in the range, for the dropdown (sorted by team-wide usage).
#[derive(Serialize, Clone)]
pub struct ModelOption {
    pub id: String,
    pub display_name: String,
    pub tokens: i64,
}

#[derive(Serialize, Clone)]
pub struct AccountOption {
    pub provider: String,
    pub account_key: String,
    pub account_label: String,
    pub tokens: i64,
    pub cost: f64,
}

#[derive(Serialize, Clone)]
pub struct TeamReport {
    pub team_name: String,
    pub range: String,
    pub members: Vec<MemberView>,
    pub models: Vec<ModelOption>,
    pub accounts: Vec<AccountOption>,
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
pub async fn upload(
    cfg: &TeamConfig,
    email: Option<&str>,
    rows: Vec<UsageRow>,
) -> Result<(), String> {
    tokio::time::timeout(UPLOAD_TIMEOUT, async {
        let mut conn = connect(cfg).await?;
        migrate(&mut conn.client).await?;
        write_member(&mut conn.client, cfg, email, &rows).await
    })
    .await
    .map_err(|_| "Team upload timed out after 60s; its transaction was rolled back.".to_string())?
}

/// Aggregate the leaderboard for `range` ("day" | "week" | "month").
pub async fn fetch(cfg: &TeamConfig, range: &str) -> Result<TeamReport, String> {
    tokio::time::timeout(FETCH_TIMEOUT, async {
        let mut conn = connect(cfg).await?;
        migrate(&mut conn.client).await?;
        read_team(&conn.client, cfg, range).await
    })
    .await
    .map_err(|_| "Team refresh timed out after 30s.".to_string())?
}

/// Same identity/visibility rules as the leaderboard, aggregated by UTC day.
/// Reads stored history only; opening a calendar never triggers a backfill.
pub async fn fetch_calendar(
    cfg: &TeamConfig,
    year: i32,
) -> Result<crate::calendar::CalendarReport, String> {
    let (start, end) = crate::calendar::year_bounds(year)?;
    tokio::time::timeout(FETCH_TIMEOUT, async {
        let mut conn = connect(cfg).await?;
        migrate(&mut conn.client).await?;
        read_calendar(&conn.client, cfg, year, &start, &end).await
    })
    .await
    .map_err(|_| "Team calendar timed out after 30s.".to_string())?
}

async fn read_calendar(
    client: &Client,
    cfg: &TeamConfig,
    year: i32,
    start: &str,
    end: &str,
) -> Result<crate::calendar::CalendarReport, String> {
    let sql = format!(
        "{USAGE_CTE}
            SELECT u.*, u.day::text AS day_text, m.display_name
            FROM readable_usage u JOIN members m ON m.member_id=u.member_id
            WHERE u.day >= $1::text::date AND u.day <= $2::text::date AND {VISIBLE_MEMBER}"
    );
    let rows = client
        .query(&sql, &[&start, &end])
        .await
        .map_err(|e| e.to_string())?;
    let pricing = crate::pricing::Pricing::loaded();
    let entries = rows
        .into_iter()
        .map(|r| {
            let model: String = r.get("model");
            let member_id: String = r.get("member_id");
            let cost = resolve_row_cost(&r, &pricing);
            crate::calendar::CalendarEntry {
                day: r.get("day_text"),
                display_name: crate::localstats::display_name(&model),
                model,
                tool: String::new(),
                member: if member_id == cfg.member_id && !cfg.display_name.is_empty() {
                    cfg.display_name.clone()
                } else {
                    r.get("display_name")
                },
                member_id,
                account_key: r.get("account_key"),
                account_label: r.get("account_label"),
                tokens: r.get("tokens"),
                cost: cost.value,
                cost_complete: cost.complete,
                cost_estimated: cost.estimated,
            }
        })
        .collect();
    Ok(crate::calendar::CalendarReport { year, entries })
}

/// The handshake: connect, migrate, write our member, read the roster back.
pub async fn handshake(
    cfg: &TeamConfig,
    email: Option<&str>,
    rows: Vec<UsageRow>,
) -> Result<TeamHandshake, String> {
    tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        let mut conn = connect(cfg).await?;
        migrate(&mut conn.client).await?;
        write_member(&mut conn.client, cfg, email, &rows).await?;
        let report = read_team(&conn.client, cfg, "day").await?;
        let members = report
            .members
            .iter()
            .map(|m| m.display_name.clone())
            .collect();
        Ok(TeamHandshake {
            ok: true,
            team_name: report.team_name,
            member_count: report.members.len(),
            members,
        })
    })
    .await
    .map_err(|_| {
        "Team connection test timed out after 75s; its transaction was rolled back.".to_string()
    })?
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
            let (h, p) = hp
                .split_once(':')
                .ok_or("HPBAR_DB_DIRECT must be host:port")?;
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

    let mut conf = tokio_postgres::Config::new();
    conf.host(&host)
        .port(port)
        .dbname(&cfg.db_name)
        .user(&cfg.db_user)
        .application_name("hpbar")
        .connect_timeout(Duration::from_secs(10));
    let (client, connection) = tokio::time::timeout(CONNECT_TIMEOUT, conf.connect(NoTls))
        .await
        .map_err(|_| "Postgres handshake timed out after 12s.".to_string())?
        .map_err(|e| format!("Postgres connect failed: {e}"))?;
    // The connection drives the protocol; it ends when `client` is dropped.
    tokio::spawn(async move {
        let _ = connection.await;
    });
    tokio::time::timeout(
        SESSION_SETUP_TIMEOUT,
        client.batch_execute(
            "SET TIME ZONE 'UTC';
             SET lock_timeout = '5s';
             SET statement_timeout = '30s';
             SET idle_in_transaction_session_timeout = '30s';",
        ),
    )
    .await
    .map_err(|_| "Postgres session setup timed out after 5s.".to_string())?
    .map_err(|e| format!("Postgres session setup failed: {e}"))?;
    Ok(Conn {
        client,
        _tunnel: tunnel,
    })
}

struct Tunnel {
    child: Child,
    port: u16,
    _stderr: crate::platform::StderrCapture,
    _helper: crate::platform::AskpassHelper,
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn open_tunnel(cfg: &TeamConfig) -> Result<Tunnel, String> {
    let port = free_local_port()?;
    let forward = format!("127.0.0.1:{port}:{}:{}", cfg.db_host, cfg.db_port);
    let helper = std::env::current_exe().map_err(|e| format!("Could not locate SSH password helper: {e}"))?;
    let helper = crate::platform::AskpassHelper::prepare(helper, !cfg.ssh_password.is_empty())?;
    let mut cmd = crate::platform::ssh_command(&cfg.ssh_host, &cfg.ssh_user, cfg.ssh_port,
        &forward, &cfg.ssh_password, &helper.path)?;
    let mut child = cmd.spawn().map_err(|e| format!(
        "Could not run SSH ({e}). Install the OpenSSH Client and check its PATH."
    ))?;

    let mut stderr = crate::platform::StderrCapture::start(&mut child);

    // Wait (≤10s) for the forwarded port to accept connections.
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return Ok(Tunnel {
                child,
                port,
                _stderr: stderr,
                _helper: helper,
            });
        }
        if let Ok(Some(status)) = child.try_wait() {
            let msg = stderr.finish();
            return Err(format!("ssh tunnel exited ({status}): {msg}"));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let _ = child.kill();
    let _ = child.wait();
    let msg = stderr.finish();
    Err(format!("ssh tunnel did not come up within 10s: {msg}"))
}

fn free_local_port() -> Result<u16, String> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| e.to_string())
}

// --- migrations + queries ----------------------------------------------------

async fn migrate(client: &mut Client) -> Result<(), String> {
    // Normal reads/uploads on an upgraded database must not contend for the
    // migration lock. Rapid range changes used to fail here while another read
    // held the lock, even though neither operation had any migration to run.
    let latest = MIGRATIONS.last().map_or(0, |(version, _)| *version);
    match client
        .query_one(
            "SELECT COALESCE(MAX(version), 0)::int FROM schema_migrations",
            &[],
        )
        .await
    {
        Ok(row) if row.get::<_, i32>(0) >= latest => return Ok(()),
        Ok(_) => {}
        Err(e) if e.code() == Some(&tokio_postgres::error::SqlState::UNDEFINED_TABLE) => {}
        Err(e) => return Err(postgres_error("schema version check failed", e)),
    }
    let tx = client.transaction().await.map_err(|e| e.to_string())?;
    // Wait for a real migrator (bounded by the session's 5s lock_timeout), then
    // recheck its committed version. Serialize initialization too, so concurrent
    // first connections cannot race on CREATE TABLE's system-catalog entries.
    tx.query_one("SELECT pg_advisory_xact_lock($1)", &[&MIGRATE_LOCK])
        .await
        .map_err(|e| postgres_error("schema migration lock failed", e))?;
    tx.batch_execute("CREATE TABLE IF NOT EXISTS schema_migrations (version int PRIMARY KEY, applied_at timestamptz DEFAULT now())")
        .await.map_err(|e| postgres_error("schema initialization failed", e))?;
    let current: i32 = tx
        .query_one(
            "SELECT COALESCE(MAX(version), 0)::int FROM schema_migrations",
            &[],
        )
        .await
        .map_err(|e| e.to_string())?
        .get(0);
    for (version, sql) in MIGRATIONS {
        if *version > current {
            tx.batch_execute(sql)
                .await
                .map_err(|e| format!("migration {version} failed: {e}"))?;
            tx.execute(
                "INSERT INTO schema_migrations(version) VALUES($1)",
                &[version],
            )
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
    let start_date = (Utc::now() - chrono::Duration::days(cfg.backfill_days.max(0))).date_naive();
    // Member identity no longer depends on an email. Keep the legacy metadata
    // column populated only when the user explicitly chose full account labels.
    let email_owned = if cfg.share_account && cfg.account_label_mode == "full" {
        email.map(str::to_string)
    } else {
        None
    };

    let tx = client.transaction().await.map_err(|e| e.to_string())?;
    // The foreground refresh and background timer can otherwise upload the
    // same installation concurrently. Fail the redundant writer quickly
    // instead of waiting behind its row locks for the whole backfill.
    let upload_lock_acquired: bool = tx
        .query_one(
            "SELECT pg_try_advisory_xact_lock(hashtextextended($1, $2))",
            &[&cfg.member_id, &MIGRATE_LOCK],
        )
        .await
        .map_err(|e| e.to_string())?
        .get(0);
    if !upload_lock_acquired {
        return Err("A Team upload for this member is already in progress.".into());
    }
    let mut legacy_member_id = cfg.legacy_member_id.clone();
    let configured_legacy_exists = if legacy_member_id.trim().is_empty() {
        false
    } else {
        tx.query_opt(
            "SELECT 1 FROM members WHERE member_id=$1 AND identity_version < 2",
            &[&legacy_member_id],
        )
        .await
        .map_err(|e| e.to_string())?
        .is_some()
    };
    if !configured_legacy_exists && !cfg.display_name.trim().is_empty() {
        // Early account-aware builds replaced the v1 id before preserving it.
        // Recover only an unambiguous same-name predecessor, and never claim a
        // legacy row already superseded by another installation.
        let candidates = tx
            .query(
                "SELECT m.member_id
                 FROM members m
                 WHERE m.identity_version < 2
                   AND lower(m.display_name)=lower($1)
                   AND NOT EXISTS (
                     SELECT 1 FROM members successor
                     WHERE successor.identity_version >= 2
                       AND successor.member_id <> $2
                       AND successor.legacy_member_id=m.member_id
                   )
                 LIMIT 2",
                &[&cfg.display_name, &cfg.member_id],
            )
            .await
            .map_err(|e| e.to_string())?;
        if candidates.len() == 1 {
            legacy_member_id = candidates[0].get("member_id");
        }
    }

    tx.execute(
        "INSERT INTO members(
           member_id, display_name, host, email, app_version, updated_at,
           current_project, identity_version, legacy_member_id
         )
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)
         ON CONFLICT (member_id) DO UPDATE SET
           display_name=excluded.display_name, host=excluded.host, email=excluded.email,
           app_version=excluded.app_version, updated_at=excluded.updated_at,
           current_project=excluded.current_project, identity_version=excluded.identity_version,
           legacy_member_id=excluded.legacy_member_id",
        &[
            &cfg.member_id,
            &cfg.display_name,
            &host,
            &email_owned,
            &app_version,
            &updated_at,
            &current_project,
            &(cfg.identity_version as i32),
            &legacy_member_id,
        ],
    )
    .await
    .map_err(|e| postgres_error("upsert member failed", e))?;

    // Replace this member's recent window wholesale (idempotent; only our rows).
    tx.execute(
        "DELETE FROM usage_daily_v2 WHERE member_id=$1 AND day >= $2",
        &[&cfg.member_id, &start_date],
    )
    .await
    .map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM usage_daily WHERE member_id=$1 AND day >= $2",
        &[&cfg.member_id, &start_date],
    )
    .await
    .map_err(|e| e.to_string())?;

    let prepared = prepare_usage_rows(cfg, rows);
    insert_usage_v2(&tx, &cfg.member_id, &prepared).await?;
    insert_legacy_usage(&tx, &cfg.member_id, &prepared).await?;

    tx.execute(
        "INSERT INTO team_meta(key, value) VALUES('team_name', $1) ON CONFLICT (key) DO NOTHING",
        &[&team_name],
    )
    .await
    .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())
}

#[derive(Hash, Eq, PartialEq)]
struct UsageKey {
    day: NaiveDate,
    provider: String,
    account_key: String,
    project: String,
    model: String,
}

struct PreparedUsage {
    key: UsageKey,
    billing_key: String,
    account_label: String,
    attribution_status: String,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_create: i64,
    unattributed: i64,
    tokens: i64,
    cost: f64,
    cost_hidden: bool,
}

/// Apply sharing controls and combine rows that privacy masking collapses onto
/// the same DB key. Pre-aggregation is also required for a multi-row Postgres
/// upsert: one statement cannot update the same conflict target twice.
fn prepare_usage_rows(cfg: &TeamConfig, rows: &[UsageRow]) -> Vec<PreparedUsage> {
    let mut prepared: HashMap<UsageKey, PreparedUsage> = HashMap::new();
    for row in rows {
        let Ok(day) = NaiveDate::parse_from_str(&row.day, "%Y-%m-%d") else {
            continue;
        };
        let project = project_label(&row.project, cfg);
        let (account_key, billing_key) =
            super::shared_account_keys(cfg, &row.account_key, &row.billing_key);
        let key = UsageKey {
            day,
            provider: row.provider.clone(),
            account_key,
            project,
            model: row.model.clone(),
        };
        let (input, output, cache_read, cache_create, unattributed, tokens) = if cfg.share_tokens {
            (
                row.input,
                row.output,
                row.cache_read,
                row.cache_create,
                row.unattributed,
                row.tokens,
            )
        } else {
            (0, 0, 0, 0, 0, 0)
        };
        let cost = if cfg.share_cost { row.cost } else { 0.0 };
        let entry = prepared.entry(key).or_insert_with_key(|key| PreparedUsage {
            key: UsageKey {
                day: key.day,
                provider: key.provider.clone(),
                account_key: key.account_key.clone(),
                project: key.project.clone(),
                model: key.model.clone(),
            },
            billing_key,
            account_label: super::shared_account_label(cfg, &row.account_label),
            attribution_status: if cfg.share_account {
                row.attribution_status.clone()
            } else {
                "hidden".into()
            },
            input: 0,
            output: 0,
            cache_read: 0,
            cache_create: 0,
            unattributed: 0,
            tokens: 0,
            cost: 0.0,
            cost_hidden: !cfg.share_cost,
        });
        entry.input += input;
        entry.output += output;
        entry.cache_read += cache_read;
        entry.cache_create += cache_create;
        entry.unattributed += unattributed;
        entry.tokens += tokens;
        entry.cost += cost;
    }
    let mut rows: Vec<_> = prepared.into_values().collect();
    rows.sort_by(|a, b| {
        a.key
            .day
            .cmp(&b.key.day)
            .then(a.key.provider.cmp(&b.key.provider))
            .then(a.key.account_key.cmp(&b.key.account_key))
            .then(a.key.project.cmp(&b.key.project))
            .then(a.key.model.cmp(&b.key.model))
    });
    rows
}

async fn insert_usage_v2(
    tx: &Transaction<'_>,
    member_id: &String,
    rows: &[PreparedUsage],
) -> Result<(), String> {
    for chunk in rows.chunks(INSERT_CHUNK_SIZE) {
        let mut sql = String::from(
            "INSERT INTO usage_daily_v2(
               member_id, day, provider, account_key, billing_key, account_label,
               attribution_status, project, model, input, output, cache_read,
               cache_create, unattributed, tokens, cost, cost_shared
             ) VALUES ",
        );
        append_placeholders(&mut sql, chunk.len(), 17);
        sql.push_str(
            " ON CONFLICT (member_id, day, provider, account_key, project, model) DO UPDATE SET
               billing_key=excluded.billing_key, account_label=excluded.account_label,
               attribution_status=excluded.attribution_status,
               input=usage_daily_v2.input + excluded.input,
               output=usage_daily_v2.output + excluded.output,
               cache_read=usage_daily_v2.cache_read + excluded.cache_read,
               cache_create=usage_daily_v2.cache_create + excluded.cache_create,
               unattributed=usage_daily_v2.unattributed + excluded.unattributed,
               tokens=usage_daily_v2.tokens + excluded.tokens,
               cost=usage_daily_v2.cost + excluded.cost,
               cost_shared=usage_daily_v2.cost_shared AND excluded.cost_shared",
        );
        let mut params: Vec<&(dyn ToSql + Sync)> = Vec::with_capacity(chunk.len() * 17);
        let shared: Vec<bool> = chunk.iter().map(|r| !r.cost_hidden).collect();
        for (row, cost_shared) in chunk.iter().zip(&shared) {
            params.extend_from_slice(&[
                member_id,
                &row.key.day,
                &row.key.provider,
                &row.key.account_key,
                &row.billing_key,
                &row.account_label,
                &row.attribution_status,
                &row.key.project,
                &row.key.model,
                &row.input,
                &row.output,
                &row.cache_read,
                &row.cache_create,
                &row.unattributed,
                &row.tokens,
                &row.cost,
                cost_shared,
            ]);
        }
        tx.execute(&sql, &params)
            .await
            .map_err(|e| postgres_error("insert usage failed", e))?;
    }
    Ok(())
}

#[derive(Hash, Eq, PartialEq)]
struct LegacyKey {
    day: NaiveDate,
    project: String,
    model: String,
}

#[derive(Default)]
struct UsageTotals {
    input: i64,
    output: i64,
    cache_read: i64,
    cache_create: i64,
    unattributed: i64,
    tokens: i64,
    cost: f64,
    cost_hidden: bool,
}

async fn insert_legacy_usage(
    tx: &Transaction<'_>,
    member_id: &String,
    rows: &[PreparedUsage],
) -> Result<(), String> {
    let mut collapsed: HashMap<LegacyKey, UsageTotals> = HashMap::new();
    for row in rows {
        let totals = collapsed
            .entry(LegacyKey {
                day: row.key.day,
                project: row.key.project.clone(),
                model: row.key.model.clone(),
            })
            .or_default();
        totals.input += row.input;
        totals.output += row.output;
        totals.cache_read += row.cache_read;
        totals.cache_create += row.cache_create;
        totals.unattributed += row.unattributed;
        totals.tokens += row.tokens;
        totals.cost += row.cost;
        totals.cost_hidden |= row.cost_hidden;
    }
    let mut collapsed: Vec<_> = collapsed.into_iter().collect();
    collapsed.sort_by(|(a, _), (b, _)| {
        a.day
            .cmp(&b.day)
            .then(a.project.cmp(&b.project))
            .then(a.model.cmp(&b.model))
    });

    for chunk in collapsed.chunks(INSERT_CHUNK_SIZE) {
        let mut sql = String::from(
            "INSERT INTO usage_daily(
               member_id, day, project, model, input, output, cache_read,
               cache_create, unattributed, tokens, cost, cost_shared
             ) VALUES ",
        );
        append_placeholders(&mut sql, chunk.len(), 12);
        sql.push_str(
            " ON CONFLICT (member_id, day, project, model) DO UPDATE SET
               input=usage_daily.input + excluded.input,
               output=usage_daily.output + excluded.output,
               cache_read=usage_daily.cache_read + excluded.cache_read,
               cache_create=usage_daily.cache_create + excluded.cache_create,
               unattributed=usage_daily.unattributed + excluded.unattributed,
               tokens=usage_daily.tokens + excluded.tokens,
               cost=usage_daily.cost + excluded.cost,
               cost_shared=usage_daily.cost_shared AND excluded.cost_shared",
        );
        let mut params: Vec<&(dyn ToSql + Sync)> = Vec::with_capacity(chunk.len() * 12);
        let shared: Vec<bool> = chunk.iter().map(|(_, t)| !t.cost_hidden).collect();
        for ((key, totals), cost_shared) in chunk.iter().zip(&shared) {
            params.extend_from_slice(&[
                member_id,
                &key.day,
                &key.project,
                &key.model,
                &totals.input,
                &totals.output,
                &totals.cache_read,
                &totals.cache_create,
                &totals.unattributed,
                &totals.tokens,
                &totals.cost,
                cost_shared,
            ]);
        }
        tx.execute(&sql, &params)
            .await
            .map_err(|e| postgres_error("insert legacy-compatible usage failed", e))?;
    }
    Ok(())
}

fn append_placeholders(sql: &mut String, rows: usize, columns: usize) {
    for row in 0..rows {
        if row > 0 {
            sql.push(',');
        }
        sql.push('(');
        for column in 0..columns {
            if column > 0 {
                sql.push(',');
            }
            sql.push('$');
            sql.push_str(&(row * columns + column + 1).to_string());
        }
        sql.push(')');
    }
}

fn postgres_error(context: &str, error: tokio_postgres::Error) -> String {
    match error.as_db_error() {
        Some(db) => format!(
            "{context}: {} (SQLSTATE {})",
            db.message(),
            db.code().code()
        ),
        None => format!("{context}: {error}"),
    }
}

fn resolve_row_cost(
    row: &tokio_postgres::Row,
    pricing: &crate::pricing::Pricing,
) -> super::cost::ResolvedCost {
    super::cost::resolve(
        pricing,
        &row.get::<_, String>("model"),
        [
            row.get("input"),
            row.get("output"),
            row.get("cache_read"),
            row.get("cache_create"),
        ],
        row.get("tokens"),
        row.get("unattributed"),
        row.get("cost"),
        row.get("cost_shared"),
    )
}

fn merge_model(models: &mut Vec<ModelUsage>, usage: &ModelUsage) {
    if let Some(existing) = models.iter_mut().find(|m| m.model == usage.model) {
        existing.tokens += usage.tokens;
        existing.cost += usage.cost;
        existing.cost_complete &= usage.cost_complete;
        existing.cost_estimated |= usage.cost_estimated;
    } else {
        models.push(usage.clone());
    }
}

async fn read_team(client: &Client, cfg: &TeamConfig, range: &str) -> Result<TeamReport, String> {
    let day_filter = match range {
        "week" => "u.day BETWEEN (now() AT TIME ZONE 'UTC')::date - 6 AND (now() AT TIME ZONE 'UTC')::date",
        "month" => "u.day BETWEEN (now() AT TIME ZONE 'UTC')::date - 29 AND (now() AT TIME ZONE 'UTC')::date",
        _ => "u.day = (now() AT TIME ZONE 'UTC')::date",
    };

    let sql = format!(
        "{USAGE_CTE}
        SELECT u.* FROM readable_usage u JOIN members m ON m.member_id=u.member_id
        WHERE {day_filter} AND {VISIBLE_MEMBER}"
    );
    let rows = client.query(&sql, &[]).await.map_err(|e| e.to_string())?;
    let pricing = crate::pricing::Pricing::loaded();
    let mut by_member: HashMap<String, Vec<ModelUsage>> = HashMap::new();
    let mut model_totals: HashMap<String, i64> = HashMap::new();
    let mut projects_by_member: HashMap<String, Vec<ProjectUsage>> = HashMap::new();
    type AccountMap = HashMap<(String, String, String, String, String), Vec<ModelUsage>>;
    let mut accounts_by_member: HashMap<String, AccountMap> = HashMap::new();
    for row in &rows {
        let member_id: String = row.get("member_id");
        let model: String = row.get("model");
        let tokens: i64 = row.get("tokens");
        let cost = resolve_row_cost(row, &pricing);
        let usage = ModelUsage {
            display_name: crate::localstats::display_name(&model),
            model: model.clone(),
            tokens,
            cost: cost.value.unwrap_or(0.0),
            cost_complete: cost.complete,
            cost_estimated: cost.estimated,
        };
        *model_totals.entry(model).or_default() += tokens;
        merge_model(by_member.entry(member_id.clone()).or_default(), &usage);
        let project: String = row.get("project");
        let projects = projects_by_member.entry(member_id.clone()).or_default();
        let index = projects
            .iter()
            .position(|p| p.project == project)
            .unwrap_or_else(|| {
                projects.push(ProjectUsage {
                    project,
                    tokens: 0,
                    cost: 0.0,
                    by_model: Vec::new(),
                });
                projects.len() - 1
            });
        projects[index].tokens += tokens;
        projects[index].cost += usage.cost;
        merge_model(&mut projects[index].by_model, &usage);
        merge_model(
            accounts_by_member
                .entry(member_id)
                .or_default()
                .entry((
                    row.get("provider"),
                    row.get("account_key"),
                    row.get("billing_key"),
                    row.get("account_label"),
                    row.get("attribution_status"),
                ))
                .or_default(),
            &usage,
        );
    }

    // Member identity/metadata (everyone, even with no usage in the range).
    let meta_sql = format!(
        "SELECT m.member_id, m.identity_version, m.display_name, m.current_project,
                    EXTRACT(EPOCH FROM updated_at)::bigint AS updated_ts
             FROM members m WHERE {VISIBLE_MEMBER}"
    );
    let meta_rows = client
        .query(&meta_sql, &[])
        .await
        .map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().timestamp();

    let mut members: Vec<MemberView> = meta_rows
        .iter()
        .map(|row| {
            let member_id: String = row.get("member_id");
            let identity_version: i32 = row.get("identity_version");
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
            let mut by_account: Vec<AccountUsage> = accounts_by_member
                .remove(&member_id)
                .unwrap_or_default()
                .into_iter()
                .map(
                    |(
                        (provider, account_key, billing_key, account_label, attribution_status),
                        mut by_model,
                    )| {
                        by_model.sort_by(|a, b| b.tokens.cmp(&a.tokens));
                        AccountUsage {
                            provider,
                            account_key,
                            billing_key,
                            account_label,
                            attribution_status,
                            tokens: by_model.iter().map(|m| m.tokens).sum(),
                            cost: by_model.iter().map(|m| m.cost).sum(),
                            by_model,
                        }
                    },
                )
                .collect();
            by_account.sort_by(|a, b| b.tokens.cmp(&a.tokens));
            let tokens = by_model.iter().map(|m| m.tokens).sum();
            let cost = by_model.iter().map(|m| m.cost).sum();
            MemberView {
                is_self: member_id == cfg.member_id,
                member_id,
                identity_version,
                is_legacy: identity_version < 2,
                display_name,
                tokens,
                cost,
                current_project: row.get("current_project"),
                last_seen_secs,
                is_stale: updated_ts == 0 || last_seen_secs > stale_secs(range),
                by_model,
                by_project,
                by_account,
            }
        })
        .collect();
    members.sort_by(|a, b| {
        b.tokens
            .cmp(&a.tokens)
            .then(a.display_name.cmp(&b.display_name))
    });

    let mut models: Vec<ModelOption> = model_totals
        .into_iter()
        .map(|(id, tokens)| ModelOption {
            display_name: crate::localstats::display_name(&id),
            id,
            tokens,
        })
        .collect();
    models.sort_by(|a, b| b.tokens.cmp(&a.tokens).then(a.id.cmp(&b.id)));

    let mut account_totals: std::collections::HashMap<(String, String), (String, i64, f64)> =
        std::collections::HashMap::new();
    for member in &members {
        if member.is_legacy {
            continue;
        }
        for account in &member.by_account {
            // A fully hidden identity cannot support a meaningful cross-member
            // bill filter, but it still appears inside that member's details.
            if matches!(account.account_key.as_str(), "hidden" | "unknown") {
                continue;
            }
            let total = account_totals
                .entry((account.provider.clone(), account.account_key.clone()))
                .or_insert_with(|| (account.account_label.clone(), 0, 0.0));
            if total.0 != account.account_label {
                total.0 = "Shared account".into();
            }
            total.1 += account.tokens;
            total.2 += account.cost;
        }
    }
    let mut accounts: Vec<AccountOption> = account_totals
        .into_iter()
        .map(
            |((provider, account_key), (account_label, tokens, cost))| AccountOption {
                provider,
                account_key,
                account_label,
                tokens,
                cost,
            },
        )
        .collect();
    accounts.sort_by(|a, b| b.tokens.cmp(&a.tokens));

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
        accounts,
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

#[cfg(test)]
mod integration_tests {
    use super::*;

    fn row(
        day: &str,
        provider: &str,
        account_key: &str,
        project: &str,
        model: &str,
        tokens: i64,
        cost: f64,
    ) -> UsageRow {
        UsageRow {
            day: day.into(),
            provider: provider.into(),
            account_key: account_key.into(),
            billing_key: format!("{provider}-billing"),
            account_label: format!("{account_key}@example.com"),
            attribution_status: "exact".into(),
            project: project.into(),
            model: model.into(),
            input: tokens,
            output: 0,
            cache_read: 0,
            cache_create: 0,
            unattributed: 0,
            tokens,
            cost,
            last_active: Utc::now().timestamp(),
        }
    }

    #[test]
    fn privacy_collisions_are_aggregated_before_batch_insert() {
        let day = Utc::now().format("%Y-%m-%d").to_string();
        let cfg = TeamConfig {
            share_project: false,
            share_account: false,
            ..TeamConfig::default()
        };
        let prepared = prepare_usage_rows(
            &cfg,
            &[
                row(
                    &day,
                    "claude",
                    "account-one",
                    "project-one",
                    "claude-fable-5",
                    50,
                    5.0,
                ),
                row(
                    &day,
                    "claude",
                    "account-two",
                    "project-two",
                    "claude-fable-5",
                    70,
                    7.0,
                ),
            ],
        );

        assert_eq!(prepared.len(), 1);
        assert_eq!(prepared[0].key.account_key, "hidden");
        assert_eq!(prepared[0].key.project, "(hidden)");
        assert_eq!(prepared[0].tokens, 120);
        assert_eq!(prepared[0].cost, 12.0);
    }

    #[test]
    fn batch_placeholders_are_numbered_across_rows() {
        let mut sql = String::new();
        append_placeholders(&mut sql, 2, 3);
        assert_eq!(sql, "($1,$2,$3),($4,$5,$6)");
    }

    /// Run explicitly against a disposable Postgres:
    /// `HPBAR_DB_DIRECT=127.0.0.1:55432 cargo test postgres_v3_rolling_upgrade_round_trip -- --ignored`
    #[tokio::test]
    #[ignore = "requires a disposable Postgres via HPBAR_DB_DIRECT"]
    async fn postgres_v3_rolling_upgrade_round_trip() {
        let day = Utc::now().format("%Y-%m-%d").to_string();
        let cfg = TeamConfig {
            enabled: true,
            team_name: "Integration Team".into(),
            db_name: "postgres".into(),
            db_user: "postgres".into(),
            member_id: "integration-member".into(),
            identity_version: 2,
            legacy_member_id: "integration-member-v1".into(),
            display_name: "Integration Member".into(),
            share_account: true,
            account_label_mode: "full".into(),
            ..TeamConfig::default()
        };

        // Reproduce the state an actual 0.8.1-beta Team DB is in before this
        // update: schema v2 already applied, with both legacy and account-aware
        // rows present. The first v3 upload below must split it safely.
        let seed = connect(&cfg).await.unwrap();
        seed.client
            .batch_execute(
                "DROP TABLE IF EXISTS usage_daily_v2, usage_daily, members, team_meta, schema_migrations CASCADE;
                 CREATE TABLE schema_migrations (
                   version int PRIMARY KEY,
                   applied_at timestamptz DEFAULT now()
                 );",
            )
            .await
            .unwrap();
        seed.client.batch_execute(MIGRATION_V1).await.unwrap();
        seed.client.batch_execute(MIGRATION_V2).await.unwrap();
        seed.client
            .batch_execute(
                "INSERT INTO schema_migrations(version) VALUES(1),(2);
                 INSERT INTO members(
                   member_id, display_name, updated_at, identity_version
                 ) VALUES
                   ('legacy-teammate','Legacy Teammate',now(),1),
                   ('integration-member-v1','Integration Member',now(),1),
                   ('lost-legacy-id','Recovered Member',now(),1),
                   ('preexisting-v2','Preexisting v2',now(),2);
                 INSERT INTO usage_daily(
                   member_id, day, provider, account_key, billing_key,
                   account_label, attribution_status, project, model, tokens, cost
                 ) VALUES
                   ('legacy-teammate',current_date,'claude','unknown','unknown',
                    'Unknown account','unknown','legacy-project','claude-fable-5',50,5),
                   ('integration-member-v1',current_date,'claude','unknown','unknown',
                    'Unknown account','unknown','legacy-project','claude-fable-5',300,30),
                   ('lost-legacy-id',current_date,'claude','unknown','unknown',
                    'Unknown account','unknown','legacy-project','claude-fable-5',10,1),
                   ('preexisting-v2',current_date,'claude','migrated-account','migrated-billing',
                    'migrated@example.com','exact','existing-project','claude-fable-5',40,4);",
            )
            .await
            .unwrap();
        drop(seed);

        // A real migration waits for an existing migrator instead of failing
        // every concurrent read/upload immediately.
        let mut migration_gate = connect(&cfg).await.unwrap();
        let gate = migration_gate.client.transaction().await.unwrap();
        gate.query_one("SELECT pg_advisory_xact_lock($1)", &[&MIGRATE_LOCK])
            .await
            .unwrap();
        let initial_upload = upload(
            &cfg,
            None,
            vec![
                row(
                    &day,
                    "claude",
                    "shared-claude",
                    "tokenhp",
                    "claude-fable-5",
                    200,
                    20.0,
                ),
                row(
                    &day,
                    "codex",
                    "shared-codex",
                    "tokenhp",
                    "gpt-5.6-sol",
                    100,
                    10.0,
                ),
            ],
        );
        let release = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            gate.commit().await.unwrap();
        };
        let (uploaded, ()) = tokio::join!(initial_upload, release);
        uploaded.unwrap();

        // Simulate an installation already migrated by the early v2 build,
        // which lost its exact v1 id. A unique display-name match recovers the
        // predecessor; a non-unique match would deliberately remain visible.
        let recovered_cfg = TeamConfig {
            member_id: "recovered-v2".into(),
            legacy_member_id: "wrong-derived-id".into(),
            display_name: "Recovered Member".into(),
            share_account: false,
            ..cfg.clone()
        };
        upload(
            &recovered_cfg,
            None,
            vec![row(
                &day,
                "claude",
                "recovered-account",
                "recovered-project",
                "claude-fable-5",
                10,
                1.0,
            )],
        )
        .await
        .unwrap();

        // Project privacy collapses two source projects into one DB key. The
        // upsert must add both rows, not let the second overwrite the first.
        let hidden_cfg = TeamConfig {
            member_id: "integration-hidden".into(),
            legacy_member_id: "integration-hidden-v1".into(),
            display_name: "Hidden Projects".into(),
            share_project: false,
            share_account: false,
            ..cfg.clone()
        };
        upload(
            &hidden_cfg,
            None,
            vec![
                row(
                    &day,
                    "claude",
                    "hidden-account",
                    "one",
                    "claude-fable-5",
                    50,
                    5.0,
                ),
                row(
                    &day,
                    "claude",
                    "hidden-account",
                    "two",
                    "claude-fable-5",
                    70,
                    7.0,
                ),
            ],
        )
        .await
        .unwrap();

        // Simulate the still-running old client uploading after v3. Its original
        // ON CONFLICT target must work against the restored legacy primary key.
        let mut conn = connect(&cfg).await.unwrap();
        migrate(&mut conn.client).await.unwrap();
        conn.client
            .execute(
                "INSERT INTO usage_daily(
                   member_id, day, project, model, tokens, cost
                 ) VALUES('legacy-teammate',$1,'legacy-project','claude-fable-5',80,8)
                 ON CONFLICT (member_id, day, project, model)
                 DO UPDATE SET tokens=excluded.tokens, cost=excluded.cost",
                &[&NaiveDate::parse_from_str(&day, "%Y-%m-%d").unwrap()],
            )
            .await
            .unwrap();
        let compatibility_tokens: i64 = conn
            .client
            .query_one(
                "SELECT COALESCE(SUM(tokens), 0)::bigint AS tokens
                 FROM usage_daily WHERE member_id=$1",
                &[&cfg.member_id],
            )
            .await
            .unwrap()
            .get("tokens");
        assert_eq!(
            compatibility_tokens, 300,
            "the v1 compatibility mirror must collapse v2 account rows"
        );
        drop(conn);

        let report = fetch(&cfg, "day").await.unwrap();
        let calendar = fetch_calendar(&cfg, day[..4].parse().unwrap())
            .await
            .unwrap();
        let own: Vec<_> = calendar
            .entries
            .iter()
            .filter(|e| e.member_id == cfg.member_id)
            .collect();
        assert_eq!(own.len(), 2);
        assert_eq!(own.iter().map(|e| e.tokens).sum::<i64>(), 300);
        assert_eq!(own.iter().filter_map(|e| e.cost).sum::<f64>(), 30.0);
        assert!(own.iter().all(|e| e.day == day));
        assert!(calendar
            .entries
            .iter()
            .any(|e| e.member_id == "legacy-teammate" && e.tokens == 80));
        assert!(!calendar
            .entries
            .iter()
            .any(|e| e.member_id == "integration-member-v1" || e.member_id == "lost-legacy-id"));
        assert_eq!(
            calendar.entries.iter().map(|e| e.tokens).sum::<i64>(),
            report.members.iter().map(|m| m.tokens).sum::<i64>()
        );
        let member = report
            .members
            .iter()
            .find(|m| m.member_id == cfg.member_id)
            .unwrap();
        assert_eq!(member.tokens, 300);
        assert!(!member.is_legacy);
        assert_eq!(member.by_account.len(), 2);
        let project = member
            .by_project
            .iter()
            .find(|p| p.project == "tokenhp")
            .unwrap();
        assert_eq!(project.tokens, 300);
        assert_eq!(project.by_model.len(), 2);
        let codex = project
            .by_model
            .iter()
            .find(|m| m.model == "gpt-5.6-sol")
            .unwrap();
        assert_eq!(codex.tokens, 100);
        assert_eq!(codex.cost, 10.0);
        assert!(codex.cost_complete);
        assert_eq!(report.accounts.len(), 3);
        assert!(report
            .members
            .iter()
            .any(|m| m.member_id == "legacy-teammate" && m.is_legacy && m.tokens == 80));
        assert!(report.members.iter().any(|m| {
            m.member_id == "preexisting-v2"
                && !m.is_legacy
                && m.tokens == 40
                && m.by_account
                    .iter()
                    .any(|a| a.account_key == "migrated-account")
        }));
        assert!(!report
            .members
            .iter()
            .any(|m| m.member_id == "integration-member-v1"));
        assert!(report
            .members
            .iter()
            .any(|m| m.member_id == recovered_cfg.member_id && !m.is_legacy));
        assert!(!report
            .members
            .iter()
            .any(|m| m.member_id == "lost-legacy-id"));

        let hidden = report
            .members
            .iter()
            .find(|m| m.member_id == hidden_cfg.member_id)
            .unwrap();
        assert_eq!(hidden.tokens, 120);
        assert_eq!(hidden.by_project.len(), 1);
        assert_eq!(hidden.by_project[0].project, "(hidden)");
        assert_eq!(hidden.by_project[0].tokens, 120);

        let history = vec![
            row(
                &day,
                "claude",
                "shared-claude",
                "tokenhp",
                "claude-fable-5",
                300,
                30.0,
            ),
            row(
                &(Utc::now() - chrono::Duration::days(3))
                    .format("%Y-%m-%d")
                    .to_string(),
                "claude",
                "shared-claude",
                "tokenhp",
                "claude-fable-5",
                50,
                5.0,
            ),
            row(
                &(Utc::now() - chrono::Duration::days(14))
                    .format("%Y-%m-%d")
                    .to_string(),
                "claude",
                "shared-claude",
                "tokenhp",
                "claude-fable-5",
                100,
                10.0,
            ),
            row(
                &(Utc::now() - chrono::Duration::days(45))
                    .format("%Y-%m-%d")
                    .to_string(),
                "claude",
                "shared-claude",
                "tokenhp",
                "claude-fable-5",
                900,
                90.0,
            ),
        ];
        upload(&cfg, None, history.clone()).await.unwrap();
        // An up-to-date schema must support rapid range reads and uploads even
        // while another connection holds the advisory migration lock.
        let mut lock_conn = connect(&cfg).await.unwrap();
        let lock = lock_conn.client.transaction().await.unwrap();
        lock.query_one("SELECT pg_advisory_xact_lock($1)", &[&MIGRATE_LOCK])
            .await
            .unwrap();
        let (day_report, week_report, month_report, uploaded) = tokio::join!(
            fetch(&cfg, "day"),
            fetch(&cfg, "week"),
            fetch(&cfg, "month"),
            upload(&cfg, None, history)
        );
        uploaded.unwrap();
        for (report, range, expected) in [
            (day_report, "day", 300),
            (week_report, "week", 350),
            (month_report, "month", 450),
        ] {
            let report = report.unwrap();
            assert_eq!(report.range, range);
            let own = report
                .members
                .iter()
                .find(|m| m.member_id == cfg.member_id)
                .unwrap();
            assert_eq!(own.tokens, expected);
            assert_eq!(
                own.by_project.iter().map(|p| p.tokens).sum::<i64>(),
                expected
            );
            assert_eq!(
                own.by_account.iter().map(|a| a.tokens).sum::<i64>(),
                expected
            );
        }
        lock.rollback().await.unwrap();

        // History from old clients has token components but no uploaded price.
        // Repricing must agree across every view/range without rewriting rows.
        let date = Utc::now().date_naive();
        let ago = |days| (date - chrono::Duration::days(days)).to_string();
        let mut partial = row(
            &day,
            "codex",
            "history",
            "partial",
            "gpt-6.1-sol",
            1_000_000,
            0.0,
        );
        partial.tokens += 123;
        partial.unattributed = 123;
        let history = vec![
            row(
                &day,
                "codex",
                "history",
                "today",
                "gpt-6.1-sol",
                1_000_000,
                0.0,
            ),
            partial,
            row(
                &ago(1),
                "codex",
                "history",
                "yesterday",
                "gpt-6.1-sol",
                2_000_000,
                0.0,
            ),
            row(
                &ago(14),
                "codex",
                "history",
                "fortnight",
                "gpt-5.6-terra",
                4_000_000,
                0.0,
            ),
            row(
                &ago(40),
                "claude",
                "older",
                "archive",
                "claude-opus-5-5",
                1_000_000,
                0.0,
            ),
            row(
                &day,
                "codex",
                "history",
                "recorded",
                "gpt-6-astra",
                1_000_000,
                20.0,
            ),
            row(
                &day,
                "codex",
                "history",
                "review",
                "codex-auto-review",
                1_000_000,
                28.6,
            ),
        ];
        upload(&cfg, None, history.clone()).await.unwrap();
        let year = chrono::Datelike::year(&date);
        // Explicit bounds also cover Jan/Feb test runs whose history crosses years.
        let conn = connect(&cfg).await.unwrap();
        let calendar = read_calendar(&conn.client, &cfg, year, &ago(45), &day)
            .await
            .unwrap();
        for (range, days, expected) in [("day", 0, 24.0), ("week", 6, 28.0), ("month", 29, 36.0)] {
            let report = fetch(&cfg, range).await.unwrap();
            let own = report.members.iter().find(|m| m.is_self).unwrap();
            assert!((own.cost - expected).abs() < 1e-9, "{range}");
            assert!((own.by_project.iter().map(|p| p.cost).sum::<f64>() - expected).abs() < 1e-9);
            assert!((own.by_account.iter().map(|a| a.cost).sum::<f64>() - expected).abs() < 1e-9);
            let cutoff = ago(days);
            let cells: Vec<_> = calendar
                .entries
                .iter()
                .filter(|e| e.member_id == cfg.member_id && e.day >= cutoff)
                .collect();
            assert_eq!(cells.iter().map(|e| e.tokens).sum::<i64>(), own.tokens);
            assert!((cells.iter().filter_map(|e| e.cost).sum::<f64>() - expected).abs() < 1e-9);
            let sol = own
                .by_model
                .iter()
                .find(|m| m.model == "gpt-6.1-sol")
                .unwrap();
            assert!(sol.cost_estimated && !sol.cost_complete && sol.cost > 0.0);
            let review = own
                .by_model
                .iter()
                .find(|m| m.model == "codex-auto-review")
                .unwrap();
            assert_eq!(review.cost, 0.0);
            assert!(!review.cost_complete);
        }
        let old = calendar
            .entries
            .iter()
            .find(|e| e.member_id == cfg.member_id && e.day == ago(40))
            .unwrap();
        assert_eq!(old.cost, Some(4.0));
        assert!(old.cost_estimated && old.cost_complete);
        assert!(calendar
            .entries
            .iter()
            .filter(|e| e.model == "codex-auto-review")
            .all(|e| e.cost.is_none()));
        let unchanged: i64 = conn
            .client
            .query_one(
                "SELECT COUNT(*)::bigint FROM usage_daily_v2 WHERE member_id=$1 AND cost=0",
                &[&cfg.member_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(
            unchanged, 5,
            "read-time estimates must not rewrite shared history"
        );

        // New privacy metadata survives writes and prevents read-time recovery.
        let private = TeamConfig {
            share_cost: false,
            ..cfg.clone()
        };
        upload(&private, None, history).await.unwrap();
        let report = fetch(&private, "month").await.unwrap();
        let own = report.members.iter().find(|m| m.is_self).unwrap();
        assert_eq!(own.cost, 0.0);
        assert!(own
            .by_model
            .iter()
            .all(|m| !m.cost_complete && !m.cost_estimated));
        let calendar = read_calendar(&conn.client, &private, year, &ago(45), &day)
            .await
            .unwrap();
        assert!(calendar
            .entries
            .iter()
            .filter(|e| e.member_id == cfg.member_id)
            .all(|e| e.cost.is_none()));
        let leaked: i64 = conn
            .client
            .query_one(
                "SELECT COUNT(*)::bigint FROM usage_daily WHERE member_id=$1 AND cost_shared",
                &[&cfg.member_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(leaked, 0, "legacy mirror must preserve cost privacy");
    }
}

#[cfg(test)]
mod pricing_audit {
    use super::*;

    #[tokio::test]
    #[ignore = "read-only audit of the configured team's model pricing"]
    async fn live_team_model_inventory() {
        let path = dirs::data_dir().unwrap().join("HPBar/team-config.json");
        let cfg: TeamConfig = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let conn = connect(&cfg).await.unwrap();
        conn.client.batch_execute("BEGIN READ ONLY").await.unwrap();
        // Audit pre-v4 servers without a migration or any persistent writes.
        let has_shared: bool = conn.client.query_one("SELECT EXISTS (
            SELECT 1 FROM information_schema.columns WHERE table_name='usage_daily_v2' AND column_name='cost_shared'
        )", &[]).await.unwrap().get(0);
        let cte = if has_shared {
            USAGE_CTE.to_string()
        } else {
            USAGE_CTE
                .replace(", cost_shared", ", true AS cost_shared")
                .replace(", u.cost_shared", ", true AS cost_shared")
        };
        let sql = format!("{cte}
            SELECT u.model, SUM(u.tokens)::bigint AS tokens,
                COUNT(*)::bigint AS rows, COUNT(*) FILTER (WHERE u.cost=0 AND u.tokens>0)::bigint AS unpriced_rows,
                SUM(u.unattributed)::bigint AS unattributed,
                SUM(u.input+u.output+u.cache_read+u.cache_create)::bigint AS components,
                MIN(u.day)::text AS first_day, MAX(u.day)::text AS last_day
            FROM readable_usage u JOIN members m ON m.member_id=u.member_id
            WHERE {VISIBLE_MEMBER} GROUP BY u.model ORDER BY tokens DESC");
        let rows = conn.client.query(&sql, &[]).await.unwrap();
        let pricing = crate::pricing::Pricing::bundled();
        let mut missing = Vec::new();
        for row in rows {
            let model: String = row.get("model");
            if pricing.cost(&model, 1, 1, 1, 1, 0).is_none() {
                missing.push(model.clone());
            }
            println!(
                "{}",
                serde_json::json!({"model": model,
                "has_bundled_rate": pricing.cost(&model, 1, 1, 1, 1, 0).is_some(),
                "tokens": row.get::<_, i64>("tokens"), "rows": row.get::<_, i64>("rows"),
                "zero_cost_rows": row.get::<_, i64>("unpriced_rows"),
                "unattributed": row.get::<_, i64>("unattributed"),
                "components": row.get::<_, i64>("components"),
                "first_day": row.get::<_, String>("first_day"), "last_day": row.get::<_, String>("last_day")})
            );
        }
        let rows = conn
            .client
            .query(
                &format!(
                    "{cte}
            SELECT u.* FROM readable_usage u JOIN members m ON m.member_id=u.member_id
            WHERE {VISIBLE_MEMBER}"
                ),
                &[],
            )
            .await
            .unwrap();
        let mut recovered = 0;
        let mut unpriced = HashMap::<String, usize>::new();
        for row in &rows {
            let c = resolve_row_cost(row, &pricing);
            let model: String = row.get("model");
            if c.estimated {
                recovered += 1;
            }
            if c.value.is_none() {
                *unpriced.entry(model.clone()).or_default() += 1;
            }
            let components: i64 = ["input", "output", "cache_read", "cache_create"]
                .iter()
                .map(|key| row.get::<_, i64>(*key))
                .sum();
            if pricing.cost(&model, 0, 0, 0, 0, 0).is_some()
                && components > 0
                && row.get::<_, bool>("cost_shared")
            {
                assert!(
                    c.value.is_some(),
                    "Recoverable row remained unpriced: {model}"
                );
            }
            if model == "codex-auto-review" {
                assert!(c.value.is_none());
            }
        }
        println!(
            "History audit: {} rows, {recovered} recovered estimates; unavailable: {unpriced:?}",
            rows.len()
        );
        conn.client.batch_execute("ROLLBACK").await.unwrap();
        assert_eq!(
            missing,
            vec!["codex-auto-review"],
            "Unexpected missing rates: {missing:?}"
        );
    }
}
