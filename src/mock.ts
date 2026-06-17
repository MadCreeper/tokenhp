// Mock data for the showcase / test tool. Activated by the `?mock` URL flag
// (see main.ts), this lets the full UI render in a plain browser — every theme
// and both tabs — with no Keychain read, no network, and no Tauri runtime.
// Run `npm run dev` and open http://localhost:1420/?mock=1 (or /showcase.html).

import type { LocalReport, UsageReport } from "./types";

export function mockLive(): UsageReport {
  const iso = (ms: number) => new Date(Date.now() + ms).toISOString();
  return {
    source_label: "Live quota",
    windows: [
      { title: "5-Hour", utilization: 0.51, remaining: 0.49, resets_at: iso((2 * 3600 + 45 * 60) * 1000), trailing: null, eta_secs: 35 * 60, machine_share: 0.34, others_share: 0.17, share_confidence: 0.82, window_budget: 42 },
      { title: "Weekly", utilization: 0.43, remaining: 0.57, resets_at: iso(26 * 3600 * 1000), trailing: null, eta_secs: null, machine_share: 0.30, others_share: 0.13, share_confidence: 0.5, window_budget: 380 },
      { title: "Extra usage", utilization: 1, remaining: 0, resets_at: null, trailing: "Off", eta_secs: null, machine_share: null, others_share: null, share_confidence: null, window_budget: null },
    ],
  };
}

const MOCK_OPUS = { id: "claude-opus-4-8", display_name: "Opus 4.8", input: 104846, output: 1665621, cache_read: 164691850, cache_create: 9091429, total: 175553746, max_component: 164691850, cost: { input: 0.52, output: 41.64, cache_read: 82.35, cache_create: 90.91, total: 215.42 } };
const MOCK_HAIKU = { id: "claude-haiku-4-5", display_name: "Haiku 4.5", input: 4231, output: 88210, cache_read: 12450000, cache_create: 410000, total: 12952441, max_component: 12450000, cost: { input: 0.0, output: 0.44, cache_read: 1.25, cache_create: 0.51, total: 2.2 } };
const MOCK_GPT = { id: "gpt-5.5", display_name: "GPT-5.5", input: 9158, output: 293, cache_read: 29312, cache_create: 0, total: 38763, max_component: 29312, cost: null };

export const MOCK_LOCAL: LocalReport = {
  source_label: "Local API usage · last 24h",
  apps: [
    { id: "claude-code", display_name: "Claude Code", kind: "equivalent", models: [MOCK_OPUS, MOCK_HAIKU], total: MOCK_OPUS.total + MOCK_HAIKU.total, cost: 217.62 },
    { id: "codex", display_name: "Codex", kind: "equivalent", models: [MOCK_GPT], total: MOCK_GPT.total, cost: null },
  ],
  combined: [MOCK_OPUS, MOCK_HAIKU, MOCK_GPT],
  projects: [
    { project: "hp_bar", tokens: 112_400_000, cost: 138.20 },
    { project: "api-gateway", tokens: 48_900_000, cost: 61.05 },
    { project: "infra", tokens: 21_300_000, cost: 14.80 },
    { project: "scratch", tokens: 5_600_000, cost: 3.55 },
  ],
};
