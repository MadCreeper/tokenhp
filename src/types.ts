/** Mirrors the Rust `AppControls` — tray-menu toggles relocated to Settings
 *  (macOS has no native tray menu since Tahoe; see lib.rs `build_tray`). */
export interface AppControls {
  autostart: boolean;
  alerts: boolean;
  calibrate: boolean;
}

/** Mirrors the Rust `usage::UsageWindow`. */
export interface UsageWindow {
  utilization: number;
  remaining: number;
  resets_at: string | null;
  title: string;
  window_minutes: number | null;
  trailing: string | null;
  /** Projected seconds until this window hits its limit at the recent burn
   *  rate — present only when that's *before* the reset (a real warning). */
  eta_secs: number | null;
  /** This machine's estimated share of the window (0..1); null until confident. */
  machine_share: number | null;
  /** Other devices' estimated share (0..1) = utilization − machine_share. */
  others_share: number | null;
  /** Fit confidence 0..1; UI hides the split below a threshold, hedges in mid. */
  share_confidence: number | null;
  /** Estimated window budget Q in local $ (tooltip/diagnostic only). */
  window_budget: number | null;
}

export interface UsageReport {
  windows: UsageWindow[];
  source_label: string;
  details: { label: string; value: string }[];
}

/** Mirrors `localstats::ModelCostDTO`. */
export interface ModelCost {
  input: number;
  output: number;
  cache_read: number;
  cache_create: number;
  total: number;
}

/** Mirrors `localstats::ModelUsageDTO`. */
export interface ModelUsage {
  id: string;
  display_name: string;
  input: number;
  output: number;
  cache_read: number;
  cache_create: number;
  unattributed: number;
  total: number;
  max_component: number;
  cost: ModelCost | null;
}

/** Mirrors `tools::AppUsageDTO` — one tool's usage. */
export interface AppUsage {
  id: string;
  display_name: string;
  /** "equivalent" (subscription priced at API rates) or "real" (API spend). */
  kind: string;
  models: ModelUsage[];
  total: number;
  cost: number | null;
}

/** Mirrors `localstats::ProjectUsageDTO` — one project's pooled tokens + cost. */
export interface ProjectUsage {
  project: string;
  tokens: number;
  cost: number;
}

/** Mirrors `tools::LocalReport` — per-tool breakdown + model-pooled total. */
export interface LocalReport {
  apps: AppUsage[];
  combined: ModelUsage[];
  /** Top projects by tokens (project-aware tools only — currently Claude Code). */
  projects: ProjectUsage[];
  source_label: string;
}

/** Mirrors `account::AccountInfo`. */
export interface Account {
  email: string | null;
  plan: string | null;
}

/** Mirrors `team::TeamConfig` — local-only, no secrets (auth is your SSH key). */
export interface TeamConfig {
  enabled: boolean;
  // SSH tunnel endpoint
  ssh_host: string;
  ssh_user: string;
  ssh_port: number;
  ssh_password: string; // optional; plaintext in config (unix only)
  // Postgres (as reached through the tunnel, i.e. the VPS's localhost)
  db_host: string;
  db_port: number;
  db_name: string;
  db_user: string;
  // identity + sharing
  team_name: string;
  member_id: string;
  identity_version: number;
  display_name: string;
  share_tokens: boolean;
  share_cost: boolean;
  share_project: boolean;
  share_account: boolean;
  account_label_mode: string;
  interval_secs: number;
  backfill_days: number;
  top_projects: number; // how many top projects to show when a row is expanded
}

/** Mirrors `team::db::ModelUsage` — one model's usage for a member. */
export interface TeamModelUsage {
  model: string;
  display_name: string;
  tokens: number;
  cost: number;
}

/** Mirrors `team::db::ProjectUsage` — one project's usage for a member. */
export interface TeamProjectUsage {
  project: string;
  tokens: number;
  cost: number;
}

/** One account bucket within a member's detailed usage ledger. */
export interface TeamAccountUsage {
  provider: string;
  account_key: string;
  billing_key: string;
  account_label: string;
  attribution_status: string;
  tokens: number;
  cost: number;
  by_model: TeamModelUsage[];
}

/** Mirrors `team::db::MemberView` — one leaderboard row. */
export interface MemberView {
  member_id: string;
  display_name: string;
  tokens: number; // total across all models
  cost: number;
  current_project: string | null;
  last_seen_secs: number;
  is_stale: boolean;
  is_self: boolean;
  by_model: TeamModelUsage[];
  by_project: TeamProjectUsage[];
  by_account: TeamAccountUsage[];
}

/** Mirrors `team::db::ModelOption` — a model for the dropdown. */
export interface TeamModelOption {
  id: string;
  display_name: string;
  tokens: number;
}

export interface TeamAccountOption {
  provider: string;
  account_key: string;
  account_label: string;
  tokens: number;
  cost: number;
}

/** Mirrors `team::db::TeamReport`. */
export interface TeamReport {
  team_name: string;
  range: string;
  members: MemberView[];
  models: TeamModelOption[];
  accounts: TeamAccountOption[];
  generated_at: string;
}

/** Mirrors `update::UpdateInfo` — the result of a check against GitHub Releases. */
export interface UpdateInfo {
  current: string; // running build's version
  latest: string; // chosen release's version (tag without the leading "v")
  latest_tag: string; // raw tag, e.g. "v0.5.1-beta"
  available: boolean; // latest is newer than current for this channel
  channel: string; // "stable" | "beta" | "alpha"
  count: number; // how many releases are eligible for this channel
  notes: string; // release body / changelog (Markdown)
  html_url: string; // the release page (fallback when no platform asset)
  published_at: string; // ISO-8601, or ""
  asset_name: string; // installer asset for this platform, or ""
  asset_url: string;
  asset_size: number;
  has_asset: boolean; // false when an update exists but ships no installer here
}

/** Mirrors `team::store::TeamHandshake`. */
export interface TeamHandshake {
  ok: boolean;
  team_name: string;
  member_count: number;
  members: string[];
}
