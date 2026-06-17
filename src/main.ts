import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/dpi";
import type {
  Account,
  LocalReport,
  ModelUsage,
  TeamConfig,
  TeamHandshake,
  TeamReport,
  UsageReport,
  UsageWindow,
} from "./types";
import { settingsContentHTML, teamContentHTML } from "./team";
import { heartsRow } from "./hearts";
import { xpBar } from "./xpbar";
import { clamp01, escapeHTML, formatDollars, formatDuration, formatTokens, nowTime } from "./util";
import { installStoneTexture } from "./texture";
import { applyTheme, cycleTheme, getTheme, isTheme, setThemeOverride, themeLabel } from "./theme";
import { classicNeutralBar, classicQuotaBar } from "./classicbar";
import { akBar, akResource } from "./arknights";
import { mockLive, MOCK_LOCAL } from "./mock";
import "./styles.css";

// Showcase / test mode: `?mock=1` feeds the UI canned data (no Keychain, no
// network, no Tauri) so it renders in a plain browser. `?theme=` and `?source=`
// force the view. Used by showcase.html and for screenshots.
const params = new URLSearchParams(location.search);
const MOCK = params.has("mock");
// In the browser there's no Tauri window to fix the width, so pin it to match.
if (MOCK) document.documentElement.style.width = "360px";

const POLL_MS = 30 * 60 * 1000; // refresh every 30 min, like the Swift app

type Source = "live" | "local" | "team";
type WindowKey = "day" | "week" | "month";
type Provider = "claude" | "codex";
type View = "main" | "settings";

const PROVIDER_KEY = "hpbar-provider";
function loadProvider(): Provider {
  return localStorage.getItem(PROVIDER_KEY) === "codex" ? "codex" : "claude";
}

const WINDOW_SECS: Record<WindowKey, number> = {
  day: 86_400,
  week: 604_800,
  month: 2_592_000,
};
const WINDOW_TITLE: Record<WindowKey, string> = { day: "24h", week: "7d", month: "30d" };

interface State {
  provider: Provider; // subscription axis (Live): whose quota
  source: Source;
  view: View; // "main" panel vs the team-settings form
  window: WindowKey;
  localTool: string; // API axis (Local): "all" or a tool id
  selectedModelId: string | null;
  dropdownOpen: boolean;
  live: UsageReport | null;
  local: LocalReport | null;
  account: Account | null;
  error: string;
  loading: boolean;
  updatedAt: string;
  // Team (opt-in)
  teamConfig: TeamConfig | null; // null until loaded; gates the Team tab
  team: TeamReport | null;
  teamRange: WindowKey;
  teamModel: string; // "all" or a model id, for the per-model leaderboard
  teamDropdownOpen: boolean;
  teamExpanded: Set<string>; // member ids whose top-projects are expanded
  teamDraft: TeamConfig | null; // edit buffer for the settings form
  teamTesting: boolean;
  teamStatus: string;
  teamStatusOk: boolean;
}

const state: State = {
  provider: loadProvider(),
  source: "live",
  view: "main",
  window: "day",
  localTool: "all",
  selectedModelId: null,
  dropdownOpen: false,
  live: null,
  local: null,
  account: null,
  error: "",
  loading: false,
  updatedAt: "",
  teamConfig: null,
  team: null,
  teamRange: "day",
  teamModel: "all",
  teamDropdownOpen: false,
  teamExpanded: new Set(),
  teamDraft: null,
  teamTesting: false,
  teamStatus: "",
  teamStatusOk: false,
};

const app = document.getElementById("app")!;

// Apply showcase URL overrides.
const themeParam = params.get("theme");
if (isTheme(themeParam)) setThemeOverride(themeParam);
const sourceParam = params.get("source");
if (sourceParam === "live" || sourceParam === "local") state.source = sourceParam;

// ---------------------------------------------------------------- data

let inFlight = false;

// Manual/auto team uploads are rate-limited so repeated refreshes (or range
// switches) can't spam SSH tunnels / the DB: at most one in flight, and no more
// than once per minute. The 30-min background uploader is separate/unaffected.
let teamUploadInFlight = false;
let lastTeamUploadAt = 0;
const TEAM_UPLOAD_MIN_MS = 60_000;

function maybeUploadTeam(): void {
  if (MOCK || !state.teamConfig?.enabled) return;
  const now = Date.now();
  if (teamUploadInFlight || now - lastTeamUploadAt < TEAM_UPLOAD_MIN_MS) return;
  teamUploadInFlight = true;
  lastTeamUploadAt = now;
  void invoke("upload_team_snapshot")
    .catch(() => {})
    .finally(() => {
      teamUploadInFlight = false;
    });
}

async function refresh(): Promise<void> {
  if (MOCK) {
    state.live = mockLive();
    state.local = MOCK_LOCAL;
    state.account = { email: "you@example.com", plan: "Max 20×" };
    state.selectedModelId = state.selectedModelId ?? MOCK_LOCAL.combined[0].id;
    state.error = "";
    state.updatedAt = nowTime();
    state.loading = false;
    render();
    return;
  }
  if (inFlight) return;
  inFlight = true;
  state.loading = true;
  render();
  const codex = state.provider === "codex";
  // Account identity rarely changes; fetch it once per provider and reuse.
  if (!state.account) {
    invoke<Account>(codex ? "fetch_codex_account" : "fetch_account")
      .then((a) => {
        state.account = a;
        render();
      })
      .catch(() => {});
  }
  try {
    if (state.source === "live") {
      // Subscription axis: the selected provider's quota.
      state.live = await invoke<UsageReport>(codex ? "fetch_codex_quota" : "fetch_usage");
    } else if (state.source === "team") {
      // Pull the shared roster; opportunistically push our own snapshot too so
      // teammates see us (fire-and-forget — the git push can be slow).
      state.team = await invoke<TeamReport>("fetch_team", { range: state.teamRange });
      // Keep the model selection valid as the range/data changes.
      if (state.teamModel !== "all" && !state.team.models.some((m) => m.id === state.teamModel)) {
        state.teamModel = "all";
      }
      maybeUploadTeam();
    } else {
      // API axis: every local tool at once (not provider-scoped).
      state.local = await invoke<LocalReport>("fetch_local", {
        windowSecs: WINDOW_SECS[state.window],
      });
      snapSelectedModel();
    }
    state.error = "";
    state.updatedAt = nowTime();
  } catch (err) {
    state.error = String(err);
  } finally {
    state.loading = false;
    inFlight = false;
    render();
  }
}

// The model set + tool kind for the current API-axis selection: the pooled
// `combined` list when "all", otherwise the chosen tool's own models.
function localModels(): { models: ModelUsage[]; kind: string | null } {
  const r = state.local;
  if (!r) return { models: [], kind: null };
  if (state.localTool === "all") return { models: r.combined, kind: null };
  const app = r.apps.find((a) => a.id === state.localTool);
  return { models: app?.models ?? [], kind: app?.kind ?? null };
}

// Keep the model selection valid as the tool/window/data changes.
function snapSelectedModel(): void {
  const { models } = localModels();
  if (!models.some((m) => m.id === state.selectedModelId)) {
    state.selectedModelId = models[0]?.id ?? null;
  }
}

// ---------------------------------------------------------------- render

const PANEL_WIDTH = 360;

function render(): void {
  app.innerHTML =
    state.view === "settings"
      ? `<main class="panel">${settingsHeaderHTML()}<section class="content">${settingsContentHTML(
          {
            draft: state.teamDraft ?? defaultTeamDraft(),
            status: state.teamStatus,
            statusOk: state.teamStatusOk,
            testing: state.teamTesting,
          },
        )}</section></main>`
      : `<main class="panel">
      ${headerHTML()}
      ${sourceSegHTML()}
      ${subSegHTML()}
      <section class="content">${contentHTML()}</section>
      ${footerHTML()}
    </main>`;
  // Resize the window to the content height (top-anchored → grows downward),
  // mirroring the AppKit panel's self-sizing.
  requestAnimationFrame(syncWindowSize);
}

// The second segmented row depends on the source; Team renders its own range
// selector inside the content area (like Local's window selector).
function subSegHTML(): string {
  if (state.source === "live") return providerSegHTML();
  if (state.source === "local") return toolSegHTML();
  return "";
}

function defaultTeamDraft(): TeamConfig {
  return (
    state.teamConfig ?? {
      enabled: false,
      ssh_host: "",
      ssh_user: "",
      ssh_port: 22,
      ssh_password: "",
      db_host: "127.0.0.1",
      db_port: 5432,
      db_name: "hpbar",
      db_user: "hpbar",
      team_name: "",
      member_id: "",
      display_name: "",
      share_tokens: true,
      share_cost: true,
      share_project: true,
      interval_secs: 1800,
      backfill_days: 90,
      top_projects: 5,
    }
  );
}

function syncWindowSize(): void {
  if (MOCK) return; // no Tauri window in browser/showcase mode
  const panel = app.querySelector(".panel") as HTMLElement | null;
  if (!panel) return;
  const h = Math.ceil(panel.getBoundingClientRect().height);
  if (h <= 1) return;
  getCurrentWindow().setSize(new LogicalSize(PANEL_WIDTH, h)).catch(() => {});
}

function headerHTML(): string {
  const title =
    state.source === "team"
      ? (state.team?.team_name ?? "Team")
      : state.source === "live"
        ? state.provider === "codex"
          ? "Codex Quota"
          : "Claude Quota"
        : "Token Usage";
  return `
    <header class="header">
      <span class="title">${escapeHTML(title)}</span>
      ${state.loading ? `<span class="spinner">…</span>` : ""}
      <button class="mc-btn icon" data-action="theme" title="Theme">${themeLabel(getTheme())}</button>
      <button class="mc-btn icon" data-action="settings" title="Team settings">⚙</button>
      <button class="mc-btn icon" data-action="refresh" title="Refresh">⟳</button>
    </header>`;
}

function settingsHeaderHTML(): string {
  return `
    <header class="header">
      <span class="title">Team Settings</span>
      <button class="mc-btn icon" data-action="theme" title="Theme">${themeLabel(getTheme())}</button>
      <button class="mc-btn icon" data-action="settings-close" title="Back">✕</button>
    </header>`;
}

function segButton(action: string, value: string, label: string, selected: boolean): string {
  return `<button class="mc-btn ${selected ? "selected" : ""}" data-action="${action}" data-value="${value}">${label}</button>`;
}

// Subscription axis (Live): which provider's quota.
function providerSegHTML(): string {
  return `
    <div class="seg provider-seg">
      ${segButton("provider", "claude", "Claude", state.provider === "claude")}
      ${segButton("provider", "codex", "Codex", state.provider === "codex")}
    </div>`;
}

// API axis (Local): which tool to show, or "All" (pooled by model). Tool options
// come from the loaded report, so the list grows as new adapters are added.
function toolSegHTML(): string {
  const apps = state.local?.apps ?? [];
  const opts = [{ id: "all", name: "All" }, ...apps.map((a) => ({ id: a.id, name: a.display_name }))];
  return `
    <div class="seg tool-seg">
      ${opts.map((o) => segButton("tool", o.id, o.name, state.localTool === o.id)).join("")}
    </div>`;
}

function sourceSegHTML(): string {
  // The Team tab only appears once the user has opted in (config.enabled).
  const teamOn = !!state.teamConfig?.enabled;
  // Shorten the first two labels when a third button has to share the row.
  const live = teamOn ? "Live" : "Live quota";
  const local = teamOn ? "Local" : "Local activity";
  return `
    <div class="seg">
      ${segButton("source", "live", live, state.source === "live")}
      ${segButton("source", "local", local, state.source === "local")}
      ${teamOn ? segButton("source", "team", "Team", state.source === "team") : ""}
    </div>`;
}

function contentHTML(): string {
  if (state.source === "team")
    return teamContentHTML({
      report: state.team,
      range: state.teamRange,
      model: state.teamModel,
      dropdownOpen: state.teamDropdownOpen,
      selfName: state.teamConfig?.display_name ?? "",
      expanded: state.teamExpanded,
      topProjects: state.teamConfig?.top_projects ?? 5,
      error: state.error,
      theme: getTheme(),
    });
  return state.source === "live" ? liveHTML() : localHTML();
}

// --- Live (hearts) ---

function liveHTML(): string {
  if (state.live) return state.live.windows.map(liveBarHTML).join("");
  if (state.error) return `<div class="msg">${escapeHTML(state.error)}</div>`;
  return `<div class="msg">Loading…</div>`;
}

function liveBarHTML(w: UsageWindow): string {
  const used = Math.round(w.utilization * 100);
  const left = Math.round(w.remaining * 100);
  const trailing = w.trailing ?? `${used}% used · ${left}% left`;
  const caption = resetCaption(w.resets_at);
  let bar: string;
  switch (getTheme()) {
    case "classic":
      bar = classicQuotaBar(w.title, w.remaining, trailing, caption);
      break;
    case "arknights":
      bar = akResource(w.title, w.remaining, caption, w.trailing);
      break;
    default:
      bar = heartBarHTML(w, trailing, caption);
  }
  // Wrap so the optional warning line stays attached to its bar and inter-window
  // spacing comes from `.live-window` (the `.bar + .bar` selectors would be broken
  // by an eta line slipped between two bars).
  return `<div class="live-window">${bar}${etaWarnHTML(w.eta_secs)}</div>`;
}

// The burn-rate projection: shown only when the backend has judged you're on pace
// to hit this window's limit *before* it resets — an actionable warning, not noise.
function etaWarnHTML(etaSecs: number | null): string {
  if (etaSecs == null) return "";
  return `<div class="bar-eta">⚠ hits limit in ~${escapeHTML(formatDuration(etaSecs))}</div>`;
}

function heartBarHTML(w: UsageWindow, trailing: string, caption: string | null): string {
  return `
    <div class="bar">
      <div class="bar-head">
        <span class="bar-title">${escapeHTML(w.title)}</span>
        <span class="bar-trailing">${escapeHTML(trailing)}</span>
      </div>
      <div class="hearts">${heartsRow(w.remaining)}</div>
      ${caption ? `<div class="bar-caption">${escapeHTML(caption)}</div>` : ""}
    </div>`;
}

function resetCaption(iso: string | null): string | null {
  if (!iso) return null;
  const ts = Date.parse(iso);
  if (Number.isNaN(ts)) return null;
  const secs = Math.round((ts - Date.now()) / 1000);
  if (secs <= 0) return "resets soon";
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (d > 0) return `resets in ${d}d ${h}h`;
  if (h > 0) return `resets in ${h}h ${m}m`;
  return `resets in ${m}m`;
}

// --- Local (XP bars) ---

function localHTML(): string {
  const windowSeg = `
    <div class="seg">
      ${(["day", "week", "month"] as WindowKey[])
        .map((w) => segButton("window", w, WINDOW_TITLE[w], state.window === w))
        .join("")}
    </div>`;

  if (!state.local) {
    const body = state.error
      ? `<div class="msg">${escapeHTML(state.error)}</div>`
      : `<div class="msg">Loading…</div>`;
    return windowSeg + body;
  }

  const { models, kind } = localModels();
  const current = models.find((m) => m.id === state.selectedModelId) ?? models[0];
  if (!current) {
    return windowSeg + `<div class="msg">No usage for this tool in this window.</div>`;
  }

  return windowSeg + kindTagHTML(kind) + dropdownHTML(models, current) + costLineHTML(current) + xpBarsHTML(current);
}

// Tag a single tool as a flat-rate subscription (cost is an API-rate estimate)
// or real metered API spend. Hidden on the pooled "All" view.
function kindTagHTML(kind: string | null): string {
  if (!kind) return "";
  const label = kind === "real" ? "real API spend" : "subscription · est. at API rates";
  return `<div class="kind-tag kind-${escapeHTML(kind)}">${label}</div>`;
}

function dropdownHTML(models: ModelUsage[], current: ModelUsage): string {
  const list = state.dropdownOpen
    ? `<div class="dd-list">${models
        .map(
          (m) =>
            `<button class="mc-btn ${m.id === current.id ? "selected" : ""}" data-action="select-model" data-value="${escapeHTML(
              m.id,
            )}">${escapeHTML(m.display_name)}</button>`,
        )
        .join("")}</div>`
    : "";
  return `
    <div class="dropdown">
      <button class="mc-btn dd-current" data-action="toggle-dropdown">
        <span>${escapeHTML(current.display_name)}</span>
        <span class="chev">${state.dropdownOpen ? "▲" : "▼"}</span>
      </button>
      ${list}
    </div>`;
}

function costLineHTML(m: ModelUsage): string {
  return `
    <div class="cost-line">
      <span class="model-id">${escapeHTML(m.id)}</span>
      ${m.cost ? `<span class="cost-total">${formatDollars(m.cost.total)}</span>` : ""}
    </div>`;
}

function xpBarsHTML(m: ModelUsage): string {
  const peak = m.max_component;
  const frac = (tokens: number) => (peak > 0 ? clamp01(tokens / peak) : 0);
  const trailing = (tokens: number, dollars: number | undefined) =>
    dollars !== undefined
      ? `${formatTokens(tokens)} · ${formatDollars(dollars)}`
      : formatTokens(tokens);
  const theme = getTheme();
  const bar =
    theme === "classic" ? classicNeutralBar : theme === "arknights" ? akBar : xpBar;
  return `
    <div class="xp-bars">
      ${bar("Input", frac(m.input), trailing(m.input, m.cost?.input))}
      ${bar("Output", frac(m.output), trailing(m.output, m.cost?.output))}
      ${bar("Cache R", frac(m.cache_read), trailing(m.cache_read, m.cost?.cache_read))}
      ${bar("Cache W", frac(m.cache_create), trailing(m.cache_create, m.cost?.cache_create))}
    </div>`;
}

function footerHTML(): string {
  const label =
    state.source === "team"
      ? "Team usage · shared DB"
      : state.source === "live"
        ? (state.live?.source_label ?? "Live quota")
        : (state.local?.source_label ?? "Local activity");
  const updated = state.updatedAt ? `Updated ${state.updatedAt}` : "";
  // The footer's middle line: account identity on Live, member count on Team.
  let midLine = "";
  if (state.source === "live") {
    // The per-account subscription (Claude login, or the ChatGPT login behind
    // Codex). Hidden on Local, which aggregates models across accounts.
    const acctText = state.account
      ? [state.account.email, state.account.plan].filter(Boolean).join(" · ")
      : "";
    if (acctText) midLine = `<div class="account">${escapeHTML(acctText)}</div>`;
  } else if (state.source === "team" && state.team) {
    const n = state.team.members.length;
    midLine = `<div class="account">${n} member${n === 1 ? "" : "s"}</div>`;
  }
  return `
    <footer class="footer">
      <div class="src-label">${escapeHTML(label)}</div>
      ${midLine}
      <div class="updated">${escapeHTML(updated)}</div>
    </footer>`;
}

// ---------------------------------------------------------------- events

app.addEventListener("click", (e) => {
  const btn = (e.target as HTMLElement).closest<HTMLElement>("[data-action]");
  if (!btn) return;
  const action = btn.dataset.action;
  const value = btn.dataset.value;

  switch (action) {
    case "refresh":
      maybeUploadTeam(); // also push our latest usage up (rate-limited)
      void refresh();
      break;
    case "provider":
      if (value && value !== state.provider) {
        state.provider = value as Provider;
        localStorage.setItem(PROVIDER_KEY, state.provider);
        // Provider only drives the subscription (Live) view + its account footer.
        state.live = null;
        state.account = null;
        state.error = "";
        render();
        void refresh();
      }
      break;
    case "tool":
      if (value && value !== state.localTool) {
        state.localTool = value;
        state.dropdownOpen = false;
        snapSelectedModel(); // re-render from already-loaded data, no refetch
        render();
      }
      break;
    case "theme":
      cycleTheme();
      syncTrayTheme(); // recolor the menu-bar heart to match
      render();
      break;
    case "source":
      if (value && value !== state.source) {
        state.source = value as Source;
        state.error = "";
        state.dropdownOpen = false;
        render(); // show cached view instantly…
        void refresh(); // …then refresh in the background
      }
      break;
    case "window":
      if (value && value !== state.window) {
        state.window = value as WindowKey;
        render();
        void refresh();
      }
      break;
    case "toggle-dropdown":
      state.dropdownOpen = !state.dropdownOpen;
      render();
      break;
    case "select-model":
      if (value) {
        state.selectedModelId = value;
        state.dropdownOpen = false;
        render();
      }
      break;
    case "team-range":
      if (value && value !== state.teamRange) {
        state.teamRange = value as WindowKey;
        state.teamDropdownOpen = false;
        render();
        void refresh();
      }
      break;
    case "team-toggle-dropdown":
      state.teamDropdownOpen = !state.teamDropdownOpen;
      render();
      break;
    case "team-select-model":
      if (value) {
        state.teamModel = value; // client-side slice — no refetch
        state.teamDropdownOpen = false;
        render();
      }
      break;
    case "team-expand":
      if (value) {
        if (state.teamExpanded.has(value)) state.teamExpanded.delete(value);
        else state.teamExpanded.add(value);
        render();
      }
      break;
    case "settings":
      openSettings();
      break;
    case "settings-close":
      state.view = "main";
      render();
      break;
    case "team-test":
      void testTeam();
      break;
    case "team-save":
      void saveTeam();
      break;
  }
});

// The settings form binds inputs to a draft buffer so typed values survive the
// re-renders triggered by Test/Save (which rebuild the whole panel).
app.addEventListener("input", (e) => {
  const el = e.target as HTMLInputElement;
  const field = el.dataset.field;
  if (!field || !state.teamDraft) return;
  const draft = state.teamDraft as unknown as Record<string, unknown>;
  draft[field] =
    el.type === "checkbox"
      ? el.checked
      : el.type === "number"
        ? Number(el.value) || 0 // ports must serialize as numbers, not strings
        : el.value;
});

function openSettings(): void {
  state.teamDraft = { ...defaultTeamDraft() };
  state.teamStatus = "";
  state.teamStatusOk = false;
  state.teamTesting = false;
  state.view = "settings";
  render();
}

async function testTeam(): Promise<void> {
  if (!state.teamDraft || state.teamTesting) return;
  state.teamTesting = true;
  state.teamStatus = "";
  render();
  try {
    const h = await invoke<TeamHandshake>("test_team_connection", { config: state.teamDraft });
    state.teamStatusOk = true;
    const n = h.member_count;
    state.teamStatus = `Connected ✓ · ${h.team_name} · ${n} member${n === 1 ? "" : "s"}`;
  } catch (err) {
    state.teamStatusOk = false;
    state.teamStatus = String(err);
  } finally {
    state.teamTesting = false;
    render();
  }
}

async function saveTeam(): Promise<void> {
  if (!state.teamDraft) return;
  try {
    await invoke("set_team_config", { config: state.teamDraft });
    state.teamConfig = await invoke<TeamConfig>("get_team_config");
    // If sharing was turned off while the Team tab was open, fall back to Live.
    if (!state.teamConfig.enabled && state.source === "team") state.source = "live";
    state.view = "main";
    state.team = null; // force a fresh fetch the next time Team is opened
    render();
    if (state.source === "team") void refresh();
  } catch (err) {
    state.teamStatusOk = false;
    state.teamStatus = String(err);
    render();
  }
}

async function loadTeamConfig(): Promise<void> {
  if (MOCK) return;
  try {
    state.teamConfig = await invoke<TeamConfig>("get_team_config");
    render();
  } catch {
    /* leave the Team tab hidden if config can't be read */
  }
}

// Push the current theme to the backend so the tray heart matches the popover
// (localStorage is the source of truth; the backend persists its own copy).
function syncTrayTheme(): void {
  if (MOCK) return;
  void invoke("set_tray_theme", { theme: getTheme() }).catch(() => {});
}

installStoneTexture();
applyTheme();
render();
void refresh();
void loadTeamConfig();
syncTrayTheme();

// Tauri-only wiring: re-fetch when the popover opens, and on a slow timer.
// Skipped in showcase/browser mode (no Tauri runtime).
if (!MOCK) {
  listen("refresh", () => void refresh());
  setInterval(() => void refresh(), POLL_MS);
}
