/** Mirrors the Rust `usage::UsageWindow`. */
export interface UsageWindow {
  utilization: number;
  remaining: number;
  resets_at: string | null;
  title: string;
  trailing: string | null;
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

/** Mirrors `tools::LocalReport` — per-tool breakdown + model-pooled total. */
export interface LocalReport {
  apps: AppUsage[];
  combined: ModelUsage[];
  source_label: string;
}

/** Mirrors `account::AccountInfo`. */
export interface Account {
  email: string | null;
  plan: string | null;
}
