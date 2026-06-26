// Team view + settings form rendering. Pure string builders (the same idiom as
// main.ts) — they take explicit args rather than importing app state, so main.ts
// stays the single owner of state.

import type { MemberView, TeamConfig, TeamReport } from "./types";
import type { Theme } from "./theme";
import { clamp01, escapeHTML, formatDollars, formatTokens } from "./util";
import { xpBar } from "./xpbar";
import { classicNeutralBar } from "./classicbar";
import { akBar } from "./arknights";

/** The model dropdown + leaderboard, for the Team tab. (The date-range segment
 *  lives in the filter line above, rendered by main.ts.) */
export function teamContentHTML(args: {
  report: TeamReport | null;
  model: string; // "all" or a model id
  dropdownOpen: boolean;
  selfName: string; // your locally-set display name (authoritative for your row)
  expanded: Set<string>; // member ids whose top-projects are shown
  topProjects: number; // how many to show when expanded
  error: string;
  theme: Theme;
}): string {
  if (!args.report) {
    return args.error
      ? `<div class="msg error">${escapeHTML(args.error)}</div>`
      : `<div class="msg">Loading…</div>`;
  }

  const report = args.report;
  if (report.members.length === 0) {
    return `<div class="msg">No members yet. Share your usage to seed the team.</div>`;
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
    .map((r, i) =>
      memberRow(i + 1, r.m, r.v.tokens, r.v.cost, max, args.theme, args.selfName, {
        open: args.expanded.has(r.m.member_id),
        topProjects: args.topProjects,
      }),
    )
    .join("");
  return dropdown + `<div class="team-list">${rows}</div>`;
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
  expand: { open: boolean; topProjects: number },
): string {
  const bar = theme === "classic" ? classicNeutralBar : theme === "arknights" ? akBar : xpBar;
  const frac = clamp01(tokens / maxTokens);
  const trailing = cost > 0 ? `${formatTokens(tokens)} · ${formatDollars(cost)}` : formatTokens(tokens);
  // Your own row uses your locally-set name, so it can't show a stale DB value.
  const display = m.is_self && selfName ? selfName : m.display_name;
  // A chevron signals the row is expandable to its top projects.
  const chevron = expand.open ? "▾" : "▸";
  const name = `${chevron} ${rank}. ${escapeHTML(display)}${m.is_self ? " (you)" : ""}`;
  const sub = [m.current_project ? `⛏ ${m.current_project}` : null, seenLabel(m)]
    .filter(Boolean)
    .join(" · ");
  return `
    <div class="team-row ${m.is_stale ? "team-stale" : ""} ${expand.open ? "team-open" : ""}"
         data-action="team-expand" data-value="${escapeHTML(m.member_id)}">
      ${bar(name, frac, trailing)}
      ${sub ? `<div class="team-sub">${escapeHTML(sub)}</div>` : ""}
      ${expand.open ? projectsHTML(m, expand.topProjects) : ""}
    </div>`;
}

// The top projects for a member (desc by tokens), shown when the row is open.
function projectsHTML(m: MemberView, topProjects: number): string {
  const list = m.by_project.slice(0, Math.max(1, topProjects));
  if (list.length === 0) {
    return `<div class="team-projects"><div class="team-proj-empty">no project activity in this range</div></div>`;
  }
  const rows = list
    .map((p) => {
      const val =
        p.cost > 0 ? `${formatTokens(p.tokens)} · ${formatDollars(p.cost)}` : formatTokens(p.tokens);
      return `<div class="team-proj"><span class="team-proj-name">${escapeHTML(
        p.project,
      )}</span><span class="team-proj-val">${val}</span></div>`;
    })
    .join("");
  return `<div class="team-projects">${rows}</div>`;
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

/** Which section of the settings form is showing. */
export type SettingsTab = "ssh" | "db" | "team";

const SETTINGS_TABS: { id: SettingsTab; label: string }[] = [
  { id: "ssh", label: "SSH" },
  { id: "db", label: "Database" },
  { id: "team", label: "Team" },
];

/** The team settings form, rendered inside `.content` of the settings view.
 *  The connection/team fields are split across tabs so the panel stays short;
 *  the master toggle and the Test/Save actions stay outside the tabs since they
 *  apply to the whole config. */
export function settingsContentHTML(args: {
  draft: TeamConfig;
  tab: SettingsTab;
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

  const tabs = SETTINGS_TABS.map(
    (t) =>
      `<button class="mc-btn ${t.id === args.tab ? "selected" : ""}" data-action="settings-tab" data-value="${t.id}">${t.label}</button>`,
  ).join("");

  return `
    <div class="settings">
      ${toggle("Enable team sharing", "enabled", d.enabled)}
      <p class="settings-help">
        Shares your token usage to a self-hosted Postgres reached over an SSH tunnel
        (no web server). Uses your existing SSH access — no DB password is stored.
        Opt-in; off by default.
      </p>

      <div class="seg settings-tabs">${tabs}</div>
      <div class="settings-pane">${tabPaneHTML(args.tab, d)}</div>

      <div class="settings-actions">
        <button class="mc-btn" data-action="team-test" ${args.testing ? "disabled" : ""}>Test Connection</button>
        <button class="mc-btn selected" data-action="team-save">Save</button>
      </div>
      ${status}
    </div>`;
}

/** The fields for the active settings tab. */
function tabPaneHTML(tab: SettingsTab, d: TeamConfig): string {
  if (tab === "ssh") {
    return `
      <div class="settings-share-title">SSH tunnel</div>
      ${field("Host", "ssh_host", d.ssh_host, "vps.example.com")}
      ${field("User", "ssh_user", d.ssh_user, "you")}
      ${field("Port", "ssh_port", String(d.ssh_port), "22", "number")}
      ${field("Password (optional)", "ssh_password", d.ssh_password, "leave blank to use SSH key", "password")}`;
  }
  if (tab === "db") {
    return `
      <div class="settings-share-title">Database (on the VPS)</div>
      ${field("Name", "db_name", d.db_name, "hpbar")}
      ${field("User", "db_user", d.db_user, "hpbar")}
      ${field("Host", "db_host", d.db_host, "127.0.0.1")}
      ${field("Port", "db_port", String(d.db_port), "5432", "number")}`;
  }
  return `
    <div class="settings-share-title">Team</div>
    ${field("Team name", "team_name", d.team_name, "My Team")}
    ${field("Your display name", "display_name", d.display_name, "")}
    ${field("Top projects (on expand)", "top_projects", String(d.top_projects), "5", "number")}

    <div class="settings-share-title">Share</div>
    <div class="settings-share">
      ${toggle("Tokens", "share_tokens", d.share_tokens)}
      ${toggle("Cost", "share_cost", d.share_cost)}
      ${toggle("Project", "share_project", d.share_project)}
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

/** A theme-aware on/off switch. Keeps a real (hidden) checkbox so the shared
 *  `input` handler in main.ts still binds it via `el.type === "checkbox"`. */
function toggle(label: string, name: string, checked: boolean): string {
  return `
    <label class="settings-toggle">
      <span class="settings-toggle-label">${escapeHTML(label)}</span>
      <input type="checkbox" data-field="${name}" ${checked ? "checked" : ""} />
      <span class="settings-toggle-track"><span class="settings-toggle-knob"></span></span>
    </label>`;
}
