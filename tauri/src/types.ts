/** Mirrors the Rust `usage::UsageWindow` serialized to the frontend. */
export interface UsageWindow {
  /** Fraction consumed, 0..1. */
  utilization: number;
  /** Fraction remaining — what the draining hearts fill to. */
  remaining: number;
  /** RFC3339 reset timestamp, if known. */
  resets_at: string | null;
  title: string;
  /** Optional trailing badge (e.g. "Off" for disabled extra usage). */
  trailing: string | null;
}

export interface UsageReport {
  windows: UsageWindow[];
  source_label: string;
}
