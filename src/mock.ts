// Mock data for the showcase / test tool. Activated by the `?mock` URL flag
// (see main.ts), this lets the full UI render in a plain browser — every theme
// and both tabs — with no Keychain read, no network, and no Tauri runtime.
// Run `npm run dev` and open http://localhost:1420/?mock=1 (or /showcase.html).

import type { LocalReport, TeamConfig, TeamReport, UsageReport } from "./types";

export function mockLive(): UsageReport {
  const iso = (ms: number) => new Date(Date.now() + ms).toISOString();
  return {
    source_label: "Live quota",
    details: [],
    windows: [
      { title: "5-Hour", window_minutes: 300, utilization: 0.51, remaining: 0.49, resets_at: iso((2 * 3600 + 45 * 60) * 1000), trailing: null, eta_secs: 35 * 60, machine_share: 0.34, others_share: 0.17, share_confidence: 0.82, window_budget: 42 },
      { title: "Weekly", window_minutes: 10080, utilization: 0.43, remaining: 0.57, resets_at: iso(26 * 3600 * 1000), trailing: null, eta_secs: null, machine_share: 0.30, others_share: 0.13, share_confidence: 0.25, window_budget: 380 },
      // Per-model weekly cap (no device-share fit for scoped windows). A
      // disabled Extra-usage window is no longer emitted at all (hidden bar).
      { title: "Weekly (Fable)", window_minutes: 10080, utilization: 0.12, remaining: 0.88, resets_at: iso(26 * 3600 * 1000), trailing: null, eta_secs: null, machine_share: null, others_share: null, share_confidence: null, window_budget: null },
    ],
  };
}

export function mockCodexLive(): UsageReport {
  const iso = (ms: number) => new Date(Date.now() + ms).toISOString();
  return {
    source_label: "Codex quota",
    details: [
      { label: "Credits", value: "1,248 remaining" },
      { label: "Plan limit", value: "Pro Lite" },
    ],
    windows: [
      {
        title: "Weekly",
        window_minutes: 10_080,
        utilization: 0.37,
        remaining: 0.63,
        resets_at: iso(3 * 24 * 3600 * 1000),
        trailing: null,
        eta_secs: 2 * 24 * 3600,
        machine_share: 0.37,
        others_share: 0,
        share_confidence: 0.88,
        window_budget: 320,
      },
    ],
  };
}

const MOCK_OPUS = { id: "claude-opus-4-8", display_name: "Opus 4.8", input: 104846, output: 1665621, cache_read: 164691850, cache_create: 9091429, unattributed: 0, total: 175553746, max_component: 164691850, cost: { input: 0.52, output: 41.64, cache_read: 82.35, cache_create: 90.91, total: 215.42 } };
const MOCK_HAIKU = { id: "claude-haiku-4-5", display_name: "Haiku 4.5", input: 4231, output: 88210, cache_read: 12450000, cache_create: 410000, unattributed: 0, total: 12952441, max_component: 12450000, cost: { input: 0.0, output: 0.44, cache_read: 1.25, cache_create: 0.51, total: 2.2 } };
const MOCK_GPT = { id: "gpt-5.5", display_name: "GPT-5.5", input: 9158, output: 293, cache_read: 29312, cache_create: 0, unattributed: 0, total: 38763, max_component: 29312, cost: null };

export const MOCK_LOCAL: LocalReport = {
  source_label: "Local API usage · last 24h",
  apps: [
    { id: "claude-code", display_name: "Claude Code", kind: "equivalent", models: [MOCK_OPUS, MOCK_HAIKU], total: MOCK_OPUS.total + MOCK_HAIKU.total, cost: 217.62 },
    { id: "codex", display_name: "Codex", kind: "equivalent", models: [MOCK_GPT], total: MOCK_GPT.total, cost: null },
  ],
  combined: [MOCK_OPUS, MOCK_HAIKU, MOCK_GPT],
  // A long list (12) so the showcase exercises the expand→scroll behavior.
  projects: [
    { project: "hp_bar", tokens: 112_400_000, cost: 138.2 },
    { project: "perf_bench_llm_tco", tokens: 48_900_000, cost: 61.05 },
    { project: "src-tauri", tokens: 21_300_000, cost: 14.8 },
    { project: "find-my-stuff", tokens: 12_700_000, cost: 9.4 },
    { project: "glm5_perf_test", tokens: 8_900_000, cost: 6.1 },
    { project: "backend", tokens: 5_600_000, cost: 3.55 },
    { project: "dotfiles", tokens: 3_900_000, cost: 2.2 },
    { project: "scratch", tokens: 2_600_000, cost: 1.5 },
    { project: "sunjichen", tokens: 1_400_000, cost: 2.49 },
    { project: "notes", tokens: 980_000, cost: 0.61 },
    { project: "infra", tokens: 540_000, cost: 0.33 },
    { project: "sandbox", tokens: 210_000, cost: 0.12 },
  ],
};

export const MOCK_TEAM_CONFIG: TeamConfig = {
  enabled: true,
  ssh_host: "team.example.com",
  ssh_user: "hpbar",
  ssh_port: 22,
  ssh_password: "",
  db_host: "127.0.0.1",
  db_port: 5432,
  db_name: "hpbar",
  db_user: "hpbar",
  team_name: "Rhodes Island",
  member_id: "member-eyja",
  identity_version: 2,
  display_name: "Eyja",
  share_tokens: true,
  share_cost: true,
  share_project: true,
  share_account: true,
  account_label_mode: "full",
  interval_secs: 1800,
  backfill_days: 90,
  top_projects: 5,
};

export const MOCK_TEAM: TeamReport = {
  team_name: "Rhodes Island",
  range: "week",
  generated_at: new Date().toISOString(),
  models: [
    { id: "claude-fable-5", display_name: "Fable 5", tokens: 680_000_000 },
    { id: "gpt-5.6-terra", display_name: "GPT-5.6 Terra", tokens: 120_000_000 },
  ],
  accounts: [
    {
      provider: "claude",
      account_key: "claude-account-shared",
      account_label: "shared@gmail.com",
      tokens: 500_000_000,
      cost: 76,
    },
    {
      provider: "claude",
      account_key: "claude-account-private",
      account_label: "private@outlook.com",
      tokens: 180_000_000,
      cost: 33,
    },
    {
      provider: "codex",
      account_key: "codex-account-team",
      account_label: "codex-team@example.com",
      tokens: 120_000_000,
      cost: 6,
    },
  ],
  members: [
    {
      member_id: "member-eyja",
      display_name: "Eyja",
      tokens: 500_000_000,
      cost: 83,
      current_project: "tokenhp",
      last_seen_secs: 45,
      is_stale: false,
      is_self: true,
      by_model: [
        { model: "claude-fable-5", display_name: "Fable 5", tokens: 400_000_000, cost: 78 },
        { model: "gpt-5.6-terra", display_name: "GPT-5.6 Terra", tokens: 100_000_000, cost: 5 },
      ],
      by_project: [{ project: "tokenhp", tokens: 360_000_000, cost: 62 }],
      by_account: [
        {
          provider: "claude",
          account_key: "claude-account-shared",
          billing_key: "claude-billing-shared",
          account_label: "shared@gmail.com",
          attribution_status: "exact",
          tokens: 220_000_000,
          cost: 45,
          by_model: [
            { model: "claude-fable-5", display_name: "Fable 5", tokens: 220_000_000, cost: 45 },
          ],
        },
        {
          provider: "claude",
          account_key: "claude-account-private",
          billing_key: "claude-billing-private",
          account_label: "private@outlook.com",
          attribution_status: "exact",
          tokens: 180_000_000,
          cost: 33,
          by_model: [
            { model: "claude-fable-5", display_name: "Fable 5", tokens: 180_000_000, cost: 33 },
          ],
        },
        {
          provider: "codex",
          account_key: "codex-account-team",
          billing_key: "codex-billing-team",
          account_label: "codex-team@example.com",
          attribution_status: "exact",
          tokens: 100_000_000,
          cost: 5,
          by_model: [
            { model: "gpt-5.6-terra", display_name: "GPT-5.6 Terra", tokens: 100_000_000, cost: 5 },
          ],
        },
      ],
    },
    {
      member_id: "member-amiya",
      display_name: "Amiya",
      tokens: 300_000_000,
      cost: 32,
      current_project: "backend",
      last_seen_secs: 320,
      is_stale: false,
      is_self: false,
      by_model: [
        { model: "claude-fable-5", display_name: "Fable 5", tokens: 280_000_000, cost: 31 },
        { model: "gpt-5.6-terra", display_name: "GPT-5.6 Terra", tokens: 20_000_000, cost: 1 },
      ],
      by_project: [{ project: "backend", tokens: 210_000_000, cost: 21 }],
      by_account: [
        {
          provider: "claude",
          account_key: "claude-account-shared",
          billing_key: "claude-billing-shared",
          account_label: "shared@gmail.com",
          attribution_status: "exact",
          tokens: 280_000_000,
          cost: 31,
          by_model: [
            { model: "claude-fable-5", display_name: "Fable 5", tokens: 280_000_000, cost: 31 },
          ],
        },
        {
          provider: "codex",
          account_key: "codex-account-team",
          billing_key: "codex-billing-team",
          account_label: "codex-team@example.com",
          attribution_status: "exact",
          tokens: 20_000_000,
          cost: 1,
          by_model: [
            { model: "gpt-5.6-terra", display_name: "GPT-5.6 Terra", tokens: 20_000_000, cost: 1 },
          ],
        },
      ],
    },
  ],
};
