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
import { heartsRow, heartsRowSplit } from "./hearts";
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
// Team uses calendar-ish labels ("Today") where Local uses durations ("24h").
const TEAM_RANGE_LABEL: Record<WindowKey, string> = { day: "Today", week: "7d", month: "30d" };

interface State {
  provider: Provider; // subscription axis (Live): whose quota
  source: Source;
  view: View; // "main" panel vs the team-settings form
  window: WindowKey;
  localTool: string; // API axis (Local): "all" or a tool id
  selectedModelId: string | null;
  dropdownOpen: boolean; // Local: the model dropdown
  toolDropdownOpen: boolean; // Local: the tool selector (All / Claude Code / …)
  providerDropdownOpen: boolean; // Live: the Claude / Codex selector
  projectsExpanded: boolean; // Local "Top projects": show all vs the top few
  showDetail: boolean; // Live: reveal the device-share text + account email/plan
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
  toolDropdownOpen: false,
  providerDropdownOpen: false,
  projectsExpanded: false,
  showDetail: false,
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
if (params.get("detail") === "1") state.showDetail = true; // showcase: pre-expand detail
if (params.get("expand") === "1") state.projectsExpanded = true; // showcase: full project list
const PACE_SHOWCASE = params.get("pace") === "1"; // showcase: force an over-pace 5-Hour

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
    if (PACE_SHOWCASE) {
      // Force the 5-Hour window over an even spend-down so the pace cue shows:
      // ~65% used at ~45% of the window elapsed → "20% over pace". eta is nulled
      // because the stronger "hits limit" warning would otherwise suppress it.
      const five = state.live.windows.find((w) => w.title === "5-Hour");
      if (five) {
        five.utilization = 0.65;
        five.remaining = 0.35;
        five.eta_secs = null;
      }
    }
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
      ${filterLineHTML()}
      <section class="content">${contentHTML()}</section>
      ${footerHTML()}
    </main>`;
  // Resize the window to the content height (top-anchored → grows downward),
  // mirroring the AppKit panel's self-sizing.
  requestAnimationFrame(syncWindowSize);
}

// The slim "filter line" under the source tabs holds each view's secondary
// controls, kept compact so the tabs stay the only chunky row. Often-toggled
// ranges are visible segments; rarely-changed tool/provider use dropdowns.
function filterLineHTML(): string {
  if (state.source === "live") return liveFilterHTML();
  if (state.source === "local") return localFilterHTML();
  if (state.source === "team") return teamFilterHTML();
  return "";
}

// Live: the Claude / Codex provider as a compact dropdown (rarely switched).
function liveFilterHTML(): string {
  const opts = [
    { id: "claude", name: "Claude" },
    { id: "codex", name: "Codex" },
  ];
  const current = opts.find((o) => o.id === state.provider) ?? opts[0];
  return (
    `<div class="filter-line">` +
    ddCurrent("toggle-provider-dropdown", current.name, state.providerDropdownOpen) +
    `</div>` +
    ddList("provider", opts, state.provider, state.providerDropdownOpen)
  );
}

// Local: tool selector (dropdown — the list grows with adapters) on the left,
// the window range (segment — toggled often) on the right.
function localFilterHTML(): string {
  const apps = state.local?.apps ?? [];
  const opts = [{ id: "all", name: "All tools" }, ...apps.map((a) => ({ id: a.id, name: a.display_name }))];
  const current = opts.find((o) => o.id === state.localTool) ?? opts[0];
  const windowSeg = `<div class="seg">${(["day", "week", "month"] as WindowKey[])
    .map((w) => segButton("window", w, WINDOW_TITLE[w], state.window === w))
    .join("")}</div>`;
  return (
    `<div class="filter-line">` +
    ddCurrent("toggle-tool-dropdown", current.name, state.toolDropdownOpen) +
    windowSeg +
    `</div>` +
    ddList("tool", opts, state.localTool, state.toolDropdownOpen)
  );
}

// Team: the date range as a segment (toggled often). The model selector stays
// in the content, next to the leaderboard it filters.
function teamFilterHTML(): string {
  if (!state.teamConfig?.enabled) return "";
  const seg = (["day", "week", "month"] as WindowKey[])
    .map((r) => segButton("team-range", r, TEAM_RANGE_LABEL[r], state.teamRange === r))
    .join("");
  return `<div class="filter-line"><div class="seg">${seg}</div></div>`;
}

// Shared dropdown bits (mirror the model dropdown markup) so the filter-line
// selectors reuse the existing .dd-current / .dd-list styling + behavior. The
// open list renders full-width *after* the filter line, not inside its flex row.
function ddCurrent(action: string, label: string, open: boolean): string {
  return `<button class="mc-btn dd-current" data-action="${action}"><span>${escapeHTML(
    label,
  )}</span><span class="chev">${open ? "▲" : "▼"}</span></button>`;
}
function ddList(
  action: string,
  opts: { id: string; name: string }[],
  selected: string,
  open: boolean,
): string {
  if (!open) return "";
  return `<div class="dd-list">${opts
    .map(
      (o) =>
        `<button class="mc-btn ${o.id === selected ? "selected" : ""}" data-action="${action}" data-value="${escapeHTML(
          o.id,
        )}">${escapeHTML(o.name)}</button>`,
    )
    .join("")}</div>`;
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
  const reset = resetCaption(w.resets_at);
  const pace = paceNote(w); // "20% over pace" on the 5-Hour bar when burning fast
  const resetText = [reset, pace].filter(Boolean).join(" · ");
  const split = machineSplit(w); // this machine's window fraction, or null when unsure
  let bar: string;
  switch (getTheme()) {
    // Caption is rendered below (in `.live-foot`), not by the theme bar, so the
    // share text can ride its right edge — pass null caption to each renderer.
    case "classic":
      bar = classicQuotaBar(w.title, w.remaining, trailing, null, split, w.trailing === "Off");
      break;
    case "arknights":
      bar = akResource(w.title, w.remaining, null, w.trailing, split);
      break;
    default:
      bar = heartBarHTML(w, trailing, null, split);
  }
  // reset countdown (left) + device-share (right) on one always-present line, so
  // toggling "detail" changes only the right text — no line added → no resize.
  const share = shareInfo(w);
  const foot =
    resetText || share.text
      ? `<div class="live-foot">
          <span class="live-reset">${resetText ? escapeHTML(resetText) : ""}</span>
          <span class="live-share${share.faint ? " faint" : ""}" title="${escapeHTML(share.title)}">${escapeHTML(share.text)}</span>
        </div>`
      : "";
  // Wrap so the foot/warning stay attached to their bar; inter-window spacing
  // comes from `.live-window` (the `.bar + .bar` selectors would be broken by a
  // line slipped between two bars).
  return `<div class="live-window">${bar}${foot}${etaWarnHTML(w.eta_secs)}</div>`;
}

// Confidence below this hides the device split entirely (bar renders as before).
const SHARE_MIN_CONF = 0.35;

// This machine's fraction of the window for the ghost split, or null when the
// fit isn't confident enough (the bar then renders normally, no ghost).
function machineSplit(w: UsageWindow): number | null {
  if (w.machine_share == null || w.share_confidence == null || w.share_confidence < SHARE_MIN_CONF) {
    return null;
  }
  return clamp01(w.machine_share);
}

// "This machine vs other devices" split for the window — an estimate from
// correlating account-wide utilization with this machine's local cost (see the
// Rust `share` module). Hidden until the fit is confident; faded in the mid range.
// The device-share text shown (via "detail") on the *right* of a bar's caption
// line, so toggling it doesn't add/remove a line (no window resize). `title` is
// the hover tooltip. Empty `text` ⇒ nothing on the right.
function shareInfo(w: UsageWindow): { text: string; faint: boolean; title: string } {
  if (!state.showDetail) return { text: "", faint: false, title: "" };
  const m = w.machine_share;
  const o = w.others_share;
  const conf = w.share_confidence;
  if (m == null || o == null || conf == null) return { text: "", faint: false, title: "" };
  if (conf < SHARE_MIN_CONF) {
    return { text: "estimating…", faint: true, title: "Device split: collecting data" };
  }
  const pct = (x: number) => Math.round(x * 100);
  const title =
    w.window_budget != null && w.window_budget > 0
      ? `≈ ${formatDollars(w.window_budget)} = 100% of this ${w.title} window (estimated)`
      : "estimated device split";
  return { text: `This machine ~${pct(m)}% · Others ~${pct(o)}%`, faint: conf < 0.6, title };
}

// The burn-rate projection: shown only when the backend has judged you're on pace
// to hit this window's limit *before* it resets — an actionable warning, not noise.
function etaWarnHTML(etaSecs: number | null): string {
  if (etaSecs == null) return "";
  return `<div class="bar-eta">⚠ hits limit in ~${escapeHTML(formatDuration(etaSecs))}</div>`;
}

function heartBarHTML(
  w: UsageWindow,
  trailing: string,
  caption: string | null,
  machineShare: number | null,
): string {
  const hearts =
    machineShare != null ? heartsRowSplit(w.remaining, machineShare) : heartsRow(w.remaining);
  return `
    <div class="bar">
      <div class="bar-head">
        <span class="bar-title">${escapeHTML(w.title)}</span>
        <span class="bar-trailing">${escapeHTML(trailing)}</span>
      </div>
      <div class="hearts">${hearts}</div>
      ${caption ? `<div class="bar-caption">${escapeHTML(caption)}</div>` : ""}
    </div>`;
}

// Even-pace check: how far along the window are you in *time* vs in *usage*?
// Only the 5-Hour window — the one you actively manage; "over pace" on the
// 7-day window early in the week is normal and not actionable.
const PACE_WINDOW_SECS: Record<string, number> = { "5-Hour": 5 * 3600 };
const PACE_THRESHOLD = 0.12; // only flag a meaningful lead (12 percentage points)

// "20% over pace" when you've burned notably more than an even spend-down would
// predict by now — a gentle nudge to slow before you hit the harder eta warning.
// null when on/under pace, unknown duration, or the eta warning is already shown.
function paceNote(w: UsageWindow): string | null {
  if (w.eta_secs != null) return null; // the stronger "hits limit" warning wins
  const dur = PACE_WINDOW_SECS[w.title];
  if (!dur || !w.resets_at) return null;
  const ts = Date.parse(w.resets_at);
  if (Number.isNaN(ts)) return null;
  const elapsed = clamp01((dur - (ts - Date.now()) / 1000) / dur); // fraction of window elapsed
  const over = w.utilization - elapsed; // >0 ⇒ ahead of an even spend-down
  if (over <= PACE_THRESHOLD) return null;
  return `${Math.round(over * 100)}% over pace`;
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
  // The window range now lives in the filter line above; content is just the
  // model breakdown for the selected tool/window.
  if (!state.local) {
    return state.error
      ? `<div class="msg">${escapeHTML(state.error)}</div>`
      : `<div class="msg">Loading…</div>`;
  }

  const { models, kind } = localModels();
  const current = models.find((m) => m.id === state.selectedModelId) ?? models[0];
  if (!current) {
    return `<div class="msg">No usage for this tool in this window.</div>`;
  }

  return (
    kindTagHTML(kind) +
    dropdownHTML(models, current) +
    costLineHTML(current) +
    xpBarsHTML(current) +
    projectsHTML()
  );
}

const PROJECTS_COLLAPSED = 4; // how many projects to show before "Show all"

// "Which repo ate my tokens" — top projects by tokens over the window, pooled
// across project-aware tools (Claude Code). Shown only in the cross-tool "All"
// view, where a project breakdown complements the by-model one. Each project
// uses the same per-theme magnitude bar as the model breakdown, so the styling
// matches the theme. Collapsed to the top few; expandable to the full list.
function projectsHTML(): string {
  if (state.localTool !== "all") return "";
  const projects = state.local?.projects ?? [];
  if (projects.length === 0) return "";
  const peak = projects[0].tokens || 1; // scale all bars to the biggest project
  const expandable = projects.length > PROJECTS_COLLAPSED;
  const shown = state.projectsExpanded ? projects : projects.slice(0, PROJECTS_COLLAPSED);

  const bar = themeBar();
  const rows = shown
    .map((p) => {
      const meta =
        p.cost > 0 ? `${formatTokens(p.tokens)} · ${formatDollars(p.cost)}` : formatTokens(p.tokens);
      return bar(p.project, clamp01(p.tokens / peak), meta);
    })
    .join("");

  const toggle = expandable
    ? `<button class="mc-btn proj-more" data-action="toggle-projects">${
        state.projectsExpanded ? "Show less ▲" : `Show all ${projects.length} ▼`
      }</button>`
    : "";
  // Wrap rows in `.xp-bars` so they get the same per-theme spacing as the model
  // breakdown (its gap applies to .xp / .cbar / .akb alike). When expanded to the
  // full list, cap the height with an inner scroll so "Show less" (below) stays
  // on-screen — otherwise a long list pushes the toggle past the popover bottom.
  const rowsClass = `xp-bars proj-rows${state.projectsExpanded ? " scroll" : ""}`;
  return `<div class="proj-section"><div class="proj-title">Top projects</div><div class="${rowsClass}">${rows}</div>${toggle}</div>`;
}

// The active theme's magnitude bar (label · trailing + bar), shared by the model
// breakdown and the project list so both render in the theme's style.
function themeBar(): (label: string, frac: number, trailing: string) => string {
  const theme = getTheme();
  return theme === "classic" ? classicNeutralBar : theme === "arknights" ? akBar : xpBar;
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
  const bar = themeBar();
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
  // The footer's middle line: account identity on Live (revealed via the footer
  // "detail" toggle), member count on Team. In live view the line is always
  // present (a blank &nbsp; placeholder when hidden) so toggling "detail" doesn't
  // grow/shrink the footer → no window resize.
  let midLine = "";
  if (state.source === "live") {
    // The per-account subscription (Claude login, or the ChatGPT login behind
    // Codex), shown only when "detail" is on. Hidden on Local, which aggregates
    // models across accounts.
    const acctText =
      state.showDetail && state.account
        ? [state.account.email, state.account.plan].filter(Boolean).join(" · ")
        : "";
    midLine = `<div class="account">${acctText ? escapeHTML(acctText) : "&nbsp;"}</div>`;
  } else if (state.source === "team" && state.team) {
    const n = state.team.members.length;
    midLine = `<div class="account">${n} member${n === 1 ? "" : "s"}</div>`;
  }
  // "detail" toggle sits next to the "Live quota" footer label (live view only) —
  // reveals the per-window device-share text + the account email/plan above.
  const detail =
    state.source === "live"
      ? ` <button class="detail-link ${state.showDetail ? "on" : ""}" data-action="detail" title="Show this-machine share + account">detail</button>`
      : "";
  return `
    <footer class="footer">
      <div class="src-label">${escapeHTML(label)}${detail}</div>
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
      state.providerDropdownOpen = false;
      if (value && value !== state.provider) {
        state.provider = value as Provider;
        localStorage.setItem(PROVIDER_KEY, state.provider);
        // Provider only drives the subscription (Live) view + its account footer.
        state.live = null;
        state.account = null;
        state.error = "";
        render();
        void refresh();
      } else {
        render(); // just closed the dropdown
      }
      break;
    case "toggle-provider-dropdown":
      state.providerDropdownOpen = !state.providerDropdownOpen;
      render();
      break;
    case "tool":
      state.toolDropdownOpen = false;
      if (value && value !== state.localTool) {
        state.localTool = value;
        state.dropdownOpen = false;
        snapSelectedModel(); // re-render from already-loaded data, no refetch
      }
      render();
      break;
    case "toggle-tool-dropdown":
      state.toolDropdownOpen = !state.toolDropdownOpen;
      state.dropdownOpen = false; // don't stack the model list under the tool list
      render();
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
        state.toolDropdownOpen = false;
        state.providerDropdownOpen = false;
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
    case "toggle-projects":
      state.projectsExpanded = !state.projectsExpanded;
      render();
      break;
    case "detail":
      state.showDetail = !state.showDetail;
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
