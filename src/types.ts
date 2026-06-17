/** Mirrors the Rust `usage::UsageWindow`. */
export interface UsageWindow {
  utilization: number;
  remaining: number;
  resets_at: string | null;
  title: string;
  trailing: string | null;
  /** Projected seconds until this window hits its limit at the recent burn
   *  rate — present only when that's *before* the reset (a real warning). */
  eta_secs: number | null;
}

export interface UsageReport {
  windows: UsageWindow[];
  source_label: string;
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
  display_name: string;
  share_tokens: boolean;
  share_cost: boolean;
  share_project: boolean;
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
}

/** Mirrors `team::db::ModelOption` — a model for the dropdown. */
export interface TeamModelOption {
  id: string;
  display_name: string;
  tokens: number;
}

/** Mirrors `team::db::TeamReport`. */
export interface TeamReport {
  team_name: string;
  range: string;
  members: MemberView[];
  models: TeamModelOption[];
  generated_at: string;
}

/** Mirrors `team::store::TeamHandshake`. */
export interface TeamHandshake {
  ok: boolean;
  team_name: string;
  member_count: number;
  members: string[];
}
