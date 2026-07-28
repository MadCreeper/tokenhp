//! Opt-in team usage sharing backed by a self-hosted Postgres reached over an
//! SSH tunnel (see [`db`]). No web server is involved — a database speaks its
//! own TCP protocol, and we never expose it publicly; clients reach it through
//! `ssh -L`, reusing the system ssh client. Config lives under
//! `{data_dir}/HPBar/team-config.json`. Key-based SSH stores no credential; if
//! the optional password field is used, that password is stored in this local
//! config file. Postgres itself is reached only through the tunnel.

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
fn default_top_projects() -> u32 {
    5
}
fn default_true_string() -> String {
    "masked".to_string()
}
fn current_identity_version() -> u32 {
    2
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
    /// v2 uses an installation UUID rather than a login email, so two people
    /// sharing one subscription account remain distinct team members.
    #[serde(default)]
    pub identity_version: u32,
    #[serde(default)]
    pub display_name: String,
    #[serde(default = "default_true")]
    pub share_tokens: bool,
    #[serde(default = "default_true")]
    pub share_cost: bool,
    #[serde(default = "default_true")]
    pub share_project: bool,
    /// Sensitive new scope: existing Team users must opt in rather than having
    /// an upgrade silently begin sharing account identifiers.
    #[serde(default)]
    pub share_account: bool,
    /// "masked" (default), "full", or "hidden".
    #[serde(default = "default_true_string")]
    pub account_label_mode: String,
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
    #[serde(default = "default_backfill")]
    pub backfill_days: i64,
    /// How many top projects to show when a leaderboard row is expanded.
    #[serde(default = "default_top_projects")]
    pub top_projects: u32,
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
            member_id: new_member_id(),
            identity_version: current_identity_version(),
            display_name: String::new(),
            share_tokens: true,
            share_cost: true,
            share_project: true,
            share_account: false,
            account_label_mode: default_true_string(),
            interval_secs: default_interval(),
            backfill_days: default_backfill(),
            top_projects: default_top_projects(),
        }
    }
}

impl TeamConfig {
    pub fn load() -> TeamConfig {
        let mut config: TeamConfig = config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if config.identity_version < current_identity_version() {
            config.member_id = new_member_id();
            config.identity_version = current_identity_version();
            let _ = config.save();
        }
        config
    }

    pub fn save(&self) -> Result<(), String> {
        let dir = config_dir().ok_or("no application data directory")?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let body = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let path = dir.join("team-config.json");
        std::fs::write(&path, body).map_err(|e| e.to_string())?;
        restrict_file_permissions(&path);
        Ok(())
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
        if self.top_projects == 0 {
            self.top_projects = default_top_projects();
        }
        if self.member_id.trim().is_empty() {
            self.member_id = new_member_id();
        }
        self.identity_version = current_identity_version();
        if self.display_name.trim().is_empty() {
            self.display_name = email
                .map(|e| e.split('@').next().unwrap_or(e).to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(hostname);
        }
        if !matches!(
            self.account_label_mode.as_str(),
            "masked" | "full" | "hidden"
        ) {
            self.account_label_mode = default_true_string();
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

#[cfg(unix)]
fn restrict_file_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_file_permissions(_path: &std::path::Path) {}

fn new_member_id() -> String {
    format!("member-{}", uuid::Uuid::new_v4())
}

pub fn shared_account_label(cfg: &TeamConfig, label: &str) -> String {
    if !cfg.share_account || cfg.account_label_mode == "hidden" {
        return "Hidden account".into();
    }
    if cfg.account_label_mode == "full" {
        return label.to_string();
    }
    mask_email(label)
}

/// Apply the account-sharing privacy toggle to stable identifiers. When account
/// sharing is off, neither the account hash nor its billing-group hash reaches
/// the team database.
pub fn shared_account_keys(
    cfg: &TeamConfig,
    account_key: &str,
    billing_key: &str,
) -> (String, String) {
    if cfg.share_account {
        (account_key.to_string(), billing_key.to_string())
    } else {
        ("hidden".into(), "hidden".into())
    }
}

fn mask_email(label: &str) -> String {
    let Some((local, domain)) = label.split_once('@') else {
        return label.to_string();
    };
    let first = local.chars().next().unwrap_or('*');
    format!("{first}***@{domain}")
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
    crate::account::observe_current_accounts();
    tokio::task::spawn_blocking(move || {
        let mut rows = crate::localstats::collect_rows(&start, &today);
        rows.extend(crate::codexstats::collect_rows(&start, &today));
        rows
    })
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
pub async fn test_connection(
    cfg: &TeamConfig,
    email: Option<&str>,
) -> Result<TeamHandshake, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_identity_is_not_derived_from_shared_email() {
        let a = TeamConfig::default();
        let b = TeamConfig::default();
        assert_eq!(a.identity_version, 2);
        assert!(a.member_id.starts_with("member-"));
        assert_ne!(a.member_id, b.member_id);
    }

    #[test]
    fn account_sharing_is_opt_in_and_labels_default_to_masked() {
        assert!(!TeamConfig::default().share_account);
        let cfg = TeamConfig {
            share_account: true,
            ..TeamConfig::default()
        };
        assert_eq!(cfg.account_label_mode, "masked");
        assert_eq!(
            shared_account_label(&cfg, "person@example.com"),
            "p***@example.com"
        );
    }

    #[test]
    fn disabling_account_share_hides_stable_keys_too() {
        let cfg = TeamConfig {
            share_account: false,
            ..TeamConfig::default()
        };
        assert_eq!(
            shared_account_keys(&cfg, "account-secret", "billing-secret"),
            ("hidden".to_string(), "hidden".to_string())
        );
    }
}
