//! Opt-in team usage sharing backed by a self-hosted Postgres reached over an
//! SSH tunnel (see [`db`]). No web server is involved — a database speaks its
//! own TCP protocol, and we never expose it publicly; clients reach it through
//! `ssh -L`, reusing the system ssh client. Config lives under
//! `{data_dir}/HPBar/team-config.json` and holds **no secrets** — the SSH key is
//! the auth and Postgres trusts the localhost tunnel.

pub mod db;

pub use db::{TeamHandshake, TeamReport};

use crate::localstats::UsageRow;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn default_ssh_port() -> u16 {
    22
}
fn default_db_host() -> String {
    "127.0.0.1".to_string()
}
fn default_db_port() -> u16 {
    5432
}
fn default_db_name() -> String {
    "hpbar".to_string()
}
fn default_db_user() -> String {
    "hpbar".to_string()
}
fn default_true() -> bool {
    true
}
fn default_interval() -> u64 {
    1800
}
fn default_backfill() -> i64 {
    90
}

/// Local-only configuration for team sharing. No secrets — auth is the user's
/// SSH key plus Postgres localhost `trust`.
#[derive(Serialize, Deserialize, Clone)]
pub struct TeamConfig {
    #[serde(default)]
    pub enabled: bool,
    // --- SSH tunnel endpoint ---
    #[serde(default)]
    pub ssh_host: String,
    #[serde(default)]
    pub ssh_user: String,
    #[serde(default = "default_ssh_port")]
    pub ssh_port: u16,
    /// Optional SSH password (unix only). Stored in the local config file in
    /// plaintext — less safe than a key; provided as an onboarding convenience.
    #[serde(default)]
    pub ssh_password: String,
    // --- Postgres (as seen from the VPS, i.e. through the tunnel) ---
    #[serde(default = "default_db_host")]
    pub db_host: String,
    #[serde(default = "default_db_port")]
    pub db_port: u16,
    #[serde(default = "default_db_name")]
    pub db_name: String,
    #[serde(default = "default_db_user")]
    pub db_user: String,
    // --- identity + sharing ---
    #[serde(default)]
    pub team_name: String,
    #[serde(default)]
    pub member_id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default = "default_true")]
    pub share_tokens: bool,
    #[serde(default = "default_true")]
    pub share_cost: bool,
    #[serde(default = "default_true")]
    pub share_project: bool,
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
    #[serde(default = "default_backfill")]
    pub backfill_days: i64,
}

impl Default for TeamConfig {
    fn default() -> Self {
        TeamConfig {
            enabled: false,
            ssh_host: String::new(),
            ssh_user: String::new(),
            ssh_port: default_ssh_port(),
            ssh_password: String::new(),
            db_host: default_db_host(),
            db_port: default_db_port(),
            db_name: default_db_name(),
            db_user: default_db_user(),
            team_name: String::new(),
            member_id: String::new(),
            display_name: String::new(),
            share_tokens: true,
            share_cost: true,
            share_project: true,
            interval_secs: default_interval(),
            backfill_days: default_backfill(),
        }
    }
}

impl TeamConfig {
    pub fn load() -> TeamConfig {
        config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let dir = config_dir().ok_or("no application data directory")?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let body = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("team-config.json"), body).map_err(|e| e.to_string())
    }

    /// Fill in derived/defaulted fields so the rest of the code can assume
    /// they're set.
    pub fn normalize(&mut self, email: Option<&str>) {
        if self.ssh_port == 0 {
            self.ssh_port = default_ssh_port();
        }
        if self.db_host.trim().is_empty() {
            self.db_host = default_db_host();
        }
        if self.db_port == 0 {
            self.db_port = default_db_port();
        }
        if self.db_name.trim().is_empty() {
            self.db_name = default_db_name();
        }
        if self.db_user.trim().is_empty() {
            self.db_user = default_db_user();
        }
        if self.interval_secs < 600 {
            self.interval_secs = default_interval();
        }
        if self.backfill_days <= 0 {
            self.backfill_days = default_backfill();
        }
        if self.member_id.trim().is_empty() {
            self.member_id = derive_member_id(email);
        }
        if self.display_name.trim().is_empty() {
            self.display_name = email
                .map(|e| e.split('@').next().unwrap_or(e).to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(hostname);
        }
    }

    /// True once there's enough to attempt a connection (or a direct-DB test).
    fn has_endpoint(&self) -> bool {
        !self.ssh_host.trim().is_empty() || std::env::var("HPBAR_DB_DIRECT").is_ok()
    }
}

fn config_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("HPBar"))
}
fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("team-config.json"))
}

fn derive_member_id(email: Option<&str>) -> String {
    let base = email.map(str::to_string).unwrap_or_else(hostname);
    let slug = slugify(&base);
    if slug.is_empty() {
        "member".to_string()
    } else {
        slug
    }
}

/// Lowercase, keep alphanumerics, collapse the rest to single dashes.
/// "jane.doe@corp.com" → "jane-doe-corp-com" (stable, identifier-safe).
fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Machine name for the member's profile. Shells out to `hostname` (present on
/// macOS, Linux and Windows).
pub fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// "YYYY-MM-DD" (UTC) for `n` days ago. Used for the upload window + queries.
pub(crate) fn day_str_back(n: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::days(n))
        .format("%Y-%m-%d")
        .to_string()
}

// --- orchestration -----------------------------------------------------------

/// Scan local logs for the upload window on a blocking thread.
async fn collect_window(cfg: &TeamConfig) -> Result<Vec<UsageRow>, String> {
    let start = day_str_back(cfg.backfill_days.max(0));
    let today = day_str_back(0);
    tokio::task::spawn_blocking(move || crate::localstats::collect_rows(&start, &today))
        .await
        .map_err(|e| e.to_string())
}

/// Push this member's identity + recent usage window to the team DB.
pub async fn upload(cfg: &TeamConfig, email: Option<&str>) -> Result<(), String> {
    if !cfg.has_endpoint() {
        return Err("SSH host is not configured.".to_string());
    }
    let rows = collect_window(cfg).await?;
    db::upload(cfg, email, rows).await
}

/// The "Test Connection" handshake: connect over the tunnel, migrate, write our
/// member, and read back the roster.
pub async fn test_connection(cfg: &TeamConfig, email: Option<&str>) -> Result<TeamHandshake, String> {
    if !cfg.has_endpoint() {
        return Err("Enter the SSH host first.".to_string());
    }
    let rows = collect_window(cfg).await?;
    db::handshake(cfg, email, rows).await
}

/// Aggregate the leaderboard for `range` ("day"|"week"|"month").
pub async fn fetch_team(range: &str) -> Result<TeamReport, String> {
    let cfg = TeamConfig::load();
    if !cfg.enabled {
        return Err("Team sync is off.".to_string());
    }
    if !cfg.has_endpoint() {
        return Err("Team sync is not configured.".to_string());
    }
    db::fetch(&cfg, range).await
}
