// Team view + settings form rendering. Pure string builders (the same idiom as
// main.ts) — they take explicit args rather than importing app state, so main.ts
// stays the single owner of state.

import type { MemberView, TeamConfig, TeamReport } from "./types";
import type { Theme } from "./theme";
import { clamp01, escapeHTML, formatDollars, formatTokens } from "./util";
import { xpBar } from "./xpbar";
import { classicNeutralBar } from "./classicbar";
import { akBar } from "./arknights";

const RANGE_LABEL: Record<string, string> = { day: "Today", week: "7d", month: "30d" };

/** The range segment + model dropdown + leaderboard, for the Team tab. */
export function teamContentHTML(args: {
  report: TeamReport | null;
  range: string;
  model: string; // "all" or a model id
  dropdownOpen: boolean;
  selfName: string; // your locally-set display name (authoritative for your row)
  error: string;
  theme: Theme;
}): string {
  const seg = `
    <div class="seg">
      ${(["day", "week", "month"] as const)
        .map(
          (r) =>
            `<button class="mc-btn ${args.range === r ? "selected" : ""}" data-action="team-range" data-value="${r}">${RANGE_LABEL[r]}</button>`,
        )
        .join("")}
    </div>`;

  if (!args.report) {
    const body = args.error
      ? `<div class="msg error">${escapeHTML(args.error)}</div>`
      : `<div class="msg">Loading…</div>`;
    return seg + body;
  }

  const report = args.report;
  if (report.members.length === 0) {
    return seg + `<div class="msg">No members yet. Share your usage to seed the team.</div>`;
  }

  const dropdown = modelDropdownHTML(report, args.model, args.dropdownOpen);

  // The value to rank/show per member: the chosen model's usage, or the total.
  const valueFor = (m: MemberView): { tokens: number; cost: number } => {
    if (args.model === "all") return { tokens: m.tokens, cost: m.cost };
    const mm = m.by_model.find((x) => x.model === args.model);
    return { tokens: mm?.tokens ?? 0, cost: mm?.cost ?? 0 };
  };

  const ranked = report.members
    .map((m) => ({ m, v: valueFor(m) }))
    .sort((a, b) => b.v.tokens - a.v.tokens);
  const max = Math.max(...ranked.map((r) => r.v.tokens), 1);
  const rows = ranked
    .map((r, i) => memberRow(i + 1, r.m, r.v.tokens, r.v.cost, max, args.theme, args.selfName))
    .join("");
  return seg + dropdown + `<div class="team-list">${rows}</div>`;
}

// The model selector — same markup as the local-usage dropdown, with its own
// action names so it switches the leaderboard client-side (no refetch).
function modelDropdownHTML(report: TeamReport, model: string, open: boolean): string {
  if (report.models.length === 0) return "";
  const current =
    model === "all"
      ? "All models"
      : (report.models.find((m) => m.id === model)?.display_name ?? "All models");
  const opts = [{ id: "all", display_name: "All models" }, ...report.models];
  const list = open
    ? `<div class="dd-list">${opts
        .map(
          (o) =>
            `<button class="mc-btn ${o.id === model ? "selected" : ""}" data-action="team-select-model" data-value="${escapeHTML(
              o.id,
            )}">${escapeHTML(o.display_name)}</button>`,
        )
        .join("")}</div>`
    : "";
  return `
    <div class="dropdown team-models">
      <button class="mc-btn dd-current" data-action="team-toggle-dropdown">
        <span>${escapeHTML(current)}</span>
        <span class="chev">${open ? "▲" : "▼"}</span>
      </button>
      ${list}
    </div>`;
}

function memberRow(
  rank: number,
  m: MemberView,
  tokens: number,
  cost: number,
  maxTokens: number,
  theme: Theme,
  selfName: string,
): string {
  const bar = theme === "classic" ? classicNeutralBar : theme === "arknights" ? akBar : xpBar;
  const frac = clamp01(tokens / maxTokens);
  const trailing = cost > 0 ? `${formatTokens(tokens)} · ${formatDollars(cost)}` : formatTokens(tokens);
  // Your own row uses your locally-set name, so it can't show a stale DB value.
  const display = m.is_self && selfName ? selfName : m.display_name;
  const name = `${rank}. ${escapeHTML(display)}${m.is_self ? " (you)" : ""}`;
  const sub = [m.current_project ? `⛏ ${m.current_project}` : null, seenLabel(m)]
    .filter(Boolean)
    .join(" · ");
  return `
    <div class="team-row ${m.is_stale ? "team-stale" : ""}">
      ${bar(name, frac, trailing)}
      ${sub ? `<div class="team-sub">${escapeHTML(sub)}</div>` : ""}
    </div>`;
}

function seenLabel(m: MemberView): string {
  const s = m.last_seen_secs;
  if (s < 0 || s > 1e8) return "no recent activity";
  if (s < 90) return "active now";
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  if (s < 86_400) return `${Math.round(s / 3600)}h ago`;
  return `${Math.round(s / 86_400)}d ago`;
}

// ---------------------------------------------------------------- settings

/** The team settings form, rendered inside `.content` of the settings view. */
export function settingsContentHTML(args: {
  draft: TeamConfig;
  status: string;
  statusOk: boolean;
  testing: boolean;
}): string {
  const d = args.draft;
  const status = args.testing
    ? `<div class="settings-status">Testing…</div>`
    : args.status
      ? `<div class="settings-status ${args.statusOk ? "ok" : "err"}">${escapeHTML(args.status)}</div>`
      : "";

  return `
    <div class="settings">
      ${check("Enable team sharing", "enabled", d.enabled)}
      <p class="settings-help">
        Shares your token usage to a self-hosted Postgres reached over an SSH tunnel
        (no web server). Uses your existing SSH access — no DB password is stored.
        Opt-in; off by default.
      </p>

      <div class="settings-share-title">SSH tunnel</div>
      ${field("Host", "ssh_host", d.ssh_host, "vps.example.com")}
      ${field("User", "ssh_user", d.ssh_user, "you")}
      ${field("Port", "ssh_port", String(d.ssh_port), "22", "number")}
      ${field("Password (optional)", "ssh_password", d.ssh_password, "leave blank to use SSH key", "password")}

      <div class="settings-share-title">Database (on the VPS)</div>
      ${field("Name", "db_name", d.db_name, "hpbar")}
      ${field("User", "db_user", d.db_user, "hpbar")}
      ${field("Host", "db_host", d.db_host, "127.0.0.1")}
      ${field("Port", "db_port", String(d.db_port), "5432", "number")}

      <div class="settings-share-title">Team</div>
      ${field("Team name", "team_name", d.team_name, "My Team")}
      ${field("Your display name", "display_name", d.display_name, "")}

      <div class="settings-share-title">Share</div>
      <div class="settings-share">
        ${check("Tokens", "share_tokens", d.share_tokens)}
        ${check("Cost", "share_cost", d.share_cost)}
        ${check("Project", "share_project", d.share_project)}
      </div>
      <div class="settings-actions">
        <button class="mc-btn" data-action="team-test" ${args.testing ? "disabled" : ""}>Test Connection</button>
        <button class="mc-btn selected" data-action="team-save">Save</button>
      </div>
      ${status}
    </div>`;
}

function field(
  label: string,
  name: string,
  value: string,
  placeholder: string,
  type: "text" | "number" | "password" = "text",
): string {
  return `
    <label class="settings-row">
      <span class="settings-label">${label}</span>
      <input class="settings-input" type="${type}" data-field="${name}"
        value="${escapeHTML(value)}" placeholder="${escapeHTML(placeholder)}"
        spellcheck="false" autocapitalize="off" autocorrect="off" autocomplete="off" />
    </label>`;
}

function check(label: string, name: string, checked: boolean): string {
  return `
    <label class="settings-check">
      <input type="checkbox" data-field="${name}" ${checked ? "checked" : ""} />
      <span>${escapeHTML(label)}</span>
    </label>`;
}
