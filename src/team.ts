// Team view + settings form rendering. Pure string builders (the same idiom as
// main.ts) — they take explicit args rather than importing app state, so main.ts
// stays the single owner of state.

import type { MemberView, TeamConfig, TeamReport, TeamModelUsage } from "./types";
import type { Theme } from "./theme";
import { clamp01, escapeHTML, formatUsageCost, formatTokens } from "./util";
import { xpBar } from "./xpbar";
import { classicNeutralBar } from "./classicbar";
import { akBar } from "./arknights";

/** The model dropdown + leaderboard, for the Team tab. (The date-range segment
 *  lives in the filter line above, rendered by main.ts.) */
export function teamContentHTML(args: {
  report: TeamReport | null;
  model: string; // "all" or a model id
  dropdownOpen: boolean;
  breakdownMode: boolean;
  accountKey: string; // "all" or a stable account key
  selfName: string; // your locally-set display name (authoritative for your row)
  expanded: Set<string>; // member ids whose top-projects are shown
  topProjects: number; // how many to show when expanded
  error: string;
  theme: Theme;
}): string {
  if (!args.report) {
    return args.error
      ? `<div class="msg"><button class="detail-link refresh-error" data-action="refresh" title="${escapeHTML(args.error)}">Retry</button></div>`
      : `<div class="msg">Loading…</div>`;
  }

  const report = args.report;
  if (report.members.length === 0) {
    return `<div class="msg">No members yet. Share your usage to seed the team.</div>`;
  }

  const modeToggle = `<button class="mc-btn breakdown-toggle ${args.breakdownMode ? "selected" : ""}"
    data-action="team-breakdown-toggle" title="Show member → account → model usage details">Usage breakdown</button>`;
  const selector = modelDropdownHTML(report, args.model, args.dropdownOpen)
    + (args.breakdownMode ? accountCycleHTML(report, args.accountKey) : "");

  // Apply the same model filter to totals, projects, accounts and share weights.
  const selectedModels = (m: MemberView): TeamModelUsage[] => {
    const models = args.breakdownMode && args.accountKey !== "all"
      ? m.by_account.filter((a) => a.account_key === args.accountKey).flatMap((a) => a.by_model)
      : m.by_model;
    return models.filter((x) => args.model === "all" || x.model === args.model);
  };
  const ranked = report.members.map((m) => ({ m, v: sumModels(selectedModels(m)) }));
  const useCost = args.breakdownMode && ranked.some((r) => r.v.cost > 0)
    && ranked.every((r) => r.v.complete);
  const weight = (v: { tokens: number; cost: number }) => (useCost ? v.cost : v.tokens);
  ranked.sort((a, b) => weight(b.v) - weight(a.v));
  const totalWeight = ranked.reduce((sum, r) => sum + weight(r.v), 0);
  const max = Math.max(...ranked.map((r) => weight(r.v)), 1);
  const rows = ranked
    .map((r, i) =>
      memberRow(
        i + 1,
        r.m,
        r.v.tokens,
        r.v.cost,
        r.v.complete,
        r.v.estimated,
        weight(r.v),
        max,
        args.theme,
        args.selfName,
        {
          open: args.expanded.has(r.m.member_id),
          topProjects: args.topProjects,
          model: args.model,
        },
        {
          enabled: args.breakdownMode,
          accountKey: args.accountKey,
          model: args.model,
          share: totalWeight > 0 ? weight(r.v) / totalWeight : 0,
        },
      ),
    )
    .join("");
  const shareHint = `Percentages: ${useCost ? "estimated cost share" : "token share (complete pricing unavailable)"}. Costs estimated at API rates. ~ reconstructed from shared token counts; + incomplete pricing.`;
  return `<div class="team-view-tools">${selector}${modeToggle}</div><div class="team-list" title="${escapeHTML(shareHint)}">${rows}</div>`;
}

function sumModels(models: TeamModelUsage[]): { tokens: number; cost: number; complete: boolean; estimated: boolean } {
  return models.reduce<{ tokens: number; cost: number; complete: boolean; estimated: boolean }>((sum, m) => ({
    tokens: sum.tokens + m.tokens,
    cost: sum.cost + m.cost,
    estimated: sum.estimated || m.cost_estimated === true,
    complete: sum.complete && m.cost_complete !== false && (m.tokens === 0 || m.cost > 0),
  }), { tokens: 0, cost: 0, complete: true, estimated: false });
}

function accountCycleHTML(report: TeamReport, accountKey: string): string {
  const current =
    accountKey === "all"
      ? { account_label: "All accounts", provider: "" }
      : (report.accounts.find((a) => a.account_key === accountKey) ?? {
          account_label: "All accounts",
          provider: "",
        });
  const prefix = current.provider ? `${providerLabel(current.provider)} · ` : "";
  return `<button class="mc-btn account-cycle" data-action="team-account-cycle"
    title="Filter bill split by account">${escapeHTML(prefix + current.account_label)}
    <span class="title-swap">⇄</span></button>`;
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
  complete: boolean,
  estimated: boolean,
  weight: number,
  maxTokens: number,
  theme: Theme,
  selfName: string,
  expand: { open: boolean; topProjects: number; model: string },
  breakdown: { enabled: boolean; accountKey: string; model: string; share: number },
): string {
  const bar = theme === "classic" ? classicNeutralBar : theme === "arknights" ? akBar : xpBar;
  const frac = clamp01(weight / maxTokens);
  const baseTrailing =
    cost > 0 ? `${formatTokens(tokens)} · ${formatUsageCost(cost, complete, estimated)}` : formatTokens(tokens);
  const trailing = breakdown.enabled ? `${baseTrailing} · ${Math.round(breakdown.share * 100)}%` : baseTrailing;
  // Your own row uses your locally-set name, so it can't show a stale DB value.
  const display = m.is_self && selfName ? selfName : m.display_name;
  // A chevron signals the row is expandable to its top projects.
  const chevron = breakdown.enabled ? "▾" : expand.open ? "▾" : "▸";
  // Theme bar builders escape their title; pass plain text to avoid turning an
  // apostrophe into a visible `&#39;` after double escaping.
  const name = `${chevron} ${rank}. ${display}${m.is_self ? " (you)" : ""}${
    m.is_legacy ? " [legacy]" : ""
  }`;
  const sub = [m.current_project ? `⛏ ${m.current_project}` : null, seenLabel(m)]
    .filter(Boolean)
    .join(" · ");
  return `
    <div class="team-row ${m.is_stale ? "team-stale" : ""} ${expand.open || breakdown.enabled ? "team-open" : ""}"
         ${breakdown.enabled ? "" : `data-action="team-expand" data-value="${escapeHTML(m.member_id)}"`}>
      ${bar(name, frac, trailing)}
      ${sub ? `<div class="team-sub">${escapeHTML(sub)}</div>` : ""}
      ${breakdown.enabled ? accountDetailsHTML(m, breakdown.accountKey, breakdown.model) : expand.open ? projectsHTML(m, expand.topProjects, expand.model) : ""}
    </div>`;
}

function accountDetailsHTML(m: MemberView, accountKey: string, model: string): string {
  const accounts = m.by_account.filter((a) => accountKey === "all" || a.account_key === accountKey)
    .map((a) => {
      const by_model = a.by_model.filter((x) => model === "all" || x.model === model);
      return { ...a, ...sumModels(by_model), by_model };
    }).filter((a) => a.by_model.length > 0);
  if (accounts.length === 0) {
    return `<div class="breakdown-records"><div class="team-proj-empty">no usage for this account</div></div>`;
  }
  const rows = accounts
    .map((account) => {
      const value =
        account.cost > 0
          ? `${formatTokens(account.tokens)} · ${formatUsageCost(account.cost, account.complete, account.estimated)}`
          : formatTokens(account.tokens);
      const models = account.by_model
        .map((model) => {
          const modelValue =
            `${formatTokens(model.tokens)} · ${formatUsageCost(model.cost,
              model.cost_complete !== false && (model.tokens === 0 || model.cost > 0), model.cost_estimated === true)}`;
          return `<div class="breakdown-model"><span>${escapeHTML(model.display_name)}</span><span>${escapeHTML(
            modelValue,
          )}</span></div>`;
        })
        .join("");
      const hidden = account.attribution_status === "hidden" || account.account_key === "hidden";
      const unknown = account.attribution_status === "unknown" || account.account_key === "unknown";
      const accountName = m.is_legacy
        ? "Legacy client · account unavailable"
        : hidden
          ? "Account not shared"
          : unknown
            ? "Unknown account · before tracking"
            : `${providerLabel(account.provider)} · ${account.account_label}`;
      const uncertain = m.is_legacy || hidden || unknown ? " breakdown-unknown" : "";
      return `<div class="breakdown-account${uncertain}">
        <div class="breakdown-account-head">
          <span>${escapeHTML(accountName)}</span>
          <span>${escapeHTML(value)}</span>
        </div>
        ${models}
      </div>`;
    })
    .join("");
  return `<div class="breakdown-records">${rows}</div>`;
}

function providerLabel(provider: string): string {
  if (provider === "claude") return "Claude";
  if (provider === "codex") return "Codex";
  return provider;
}

// The top projects for a member (desc by tokens), shown when the row is open.
function projectsHTML(m: MemberView, topProjects: number, model: string): string {
  const list = m.by_project.map((p) => ({
    ...p, ...sumModels((p.by_model ?? []).filter((x) => model === "all" || x.model === model)),
  })).filter((p) => p.tokens > 0 || p.cost > 0)
    .sort((a, b) => b.tokens - a.tokens || a.project.localeCompare(b.project))
    .slice(0, Math.max(1, topProjects));
  if (list.length === 0) {
    return `<div class="team-projects"><div class="team-proj-empty">no project activity in this range</div></div>`;
  }
  const rows = list
    .map((p) => {
      const val =
        p.cost > 0 ? `${formatTokens(p.tokens)} · ${formatUsageCost(p.cost, p.complete, p.estimated)}` : formatTokens(p.tokens);
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
    ${selectField("Account labels", "account_label_mode", d.account_label_mode, [
      ["masked", "Masked email"],
      ["full", "Full email"],
      ["hidden", "Hidden"],
    ])}

    <div class="settings-share-title">Share</div>
    <p class="settings-help">Account is off by default. When enabled, labels use the privacy mode above.</p>
    <div class="settings-share">
      ${toggle("Tokens", "share_tokens", d.share_tokens)}
      ${toggle("Cost", "share_cost", d.share_cost)}
      ${toggle("Project", "share_project", d.share_project)}
      ${toggle("Account", "share_account", d.share_account)}
    </div>`;
}

function selectField(
  label: string,
  name: string,
  value: string,
  options: [string, string][],
): string {
  return `
    <label class="settings-row">
      <span class="settings-label">${escapeHTML(label)}</span>
      <select class="settings-input" data-field="${name}">
        ${options
          .map(
            ([id, text]) =>
              `<option value="${escapeHTML(id)}" ${id === value ? "selected" : ""}>${escapeHTML(text)}</option>`,
          )
          .join("")}
      </select>
    </label>`;
}

function field(
  label: string,
  name: string,
  value: string,
  placeholder: string,
  type: "text" | "number" | "password" = "text",
): string {
  const id = `team-${name}`;
  const input = `<input id="${id}" class="settings-input" type="${type}" data-field="${name}"
    value="${escapeHTML(value)}" placeholder="${escapeHTML(placeholder)}"
    spellcheck="false" autocapitalize="off" autocorrect="off" autocomplete="off" />`;
  if (type === "password") {
    return `<div class="settings-row">
      <label class="settings-label" for="${id}">${escapeHTML(label)}</label>
      <div class="settings-password">${input}<button type="button" class="mc-btn settings-password-toggle"
        data-action="team-password-toggle" aria-controls="${id}" aria-label="Show SSH password" aria-pressed="false">Show</button></div>
    </div>`;
  }
  return `
    <label class="settings-row">
      <span class="settings-label">${label}</span>
      ${input}
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
