import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/dpi";
import type {
  Account,
  AppControls,
  LocalReport,
  ModelUsage,
  TeamConfig,
  TeamHandshake,
  TeamReport,
  UpdateInfo,
  UsageReport,
  UsageWindow,
} from "./types";
import { settingsContentHTML, teamContentHTML, type SettingsTab } from "./team";
import {
  aboutSectionHTML,
  generalSectionHTML,
  updateSectionHTML,
  type SettingsSection,
  type UpdateChannel,
} from "./about";
import { pinIcon, refreshIcon, settingsIcon } from "./icons";
import { heartsRow, heartsRowSplit, heartsRefillHTML } from "./hearts";
import { xpBar } from "./xpbar";
import { clamp01, escapeHTML, formatDollars, formatDuration, formatTokens, nowTime } from "./util";
import { installStoneTexture } from "./texture";
import { applyTheme, cycleTheme, getTheme, isTheme, setThemeOverride, themeLabel } from "./theme";
import { classicNeutralBar, classicQuotaBar, classicRefillBar } from "./classicbar";
import { akBar, akResource, akRefillBar } from "./arknights";
import { mockCodexLive, mockLive, MOCK_LOCAL, MOCK_TEAM, MOCK_TEAM_CONFIG } from "./mock";
import "./styles.css";

// Showcase / test mode: `?mock=1` feeds the UI canned data (no Keychain, no
// network, no Tauri) so it renders in a plain browser. `?theme=` and `?source=`
// force the view. Used by showcase.html and for screenshots.
const params = new URLSearchParams(location.search);
const MOCK = params.has("mock");
// macOS has no native tray menu (Tahoe would show it on left-click), so the
// popover hosts its controls in a "General" settings section. Linux/Windows keep
// the native tray menu, so they don't show that section. The webview UA reliably
// carries the OS; `?os=mac` forces it on for showcase pages.
const IS_MAC = /Mac/i.test(navigator.userAgent) || params.get("os") === "mac";
// In the browser there's no Tauri window to fix the width, so pin it to match.
if (MOCK) document.documentElement.style.width = "360px";

// Liquid Glass look (macOS only), user-toggleable in Settings → General.
// Default on. Two <html> classes gate the CSS:
//   `mac`   — the popover window is transparent on macOS, so the classic body
//             is transparent and the rounded panel defines the window shape
//             (rounded corners) whether or not glass is on.
//   `glass` — the glass styling is active: panel goes translucent to ride on the
//             native NSGlassEffectView backdrop. Off → panel stays opaque (a
//             plain rounded popover) and the native view is hidden (set_glass_enabled).
// The `blurred` class (focused↔unfocused widget states) is toggled from Rust in
// build_popover's Focused handler via eval — DOM focus/blur and onFocusChanged
// both proved unreliable in the WKWebView; the Rust window event is the one that
// fires. applyTheme() rewrites body.className wholesale, so these live on <html>.
const GLASS_KEY = "hpbar-glass";
function glassEnabled(): boolean {
  return localStorage.getItem(GLASS_KEY) !== "0"; // default on
}
if (IS_MAC && !MOCK) {
  const root = document.documentElement;
  root.classList.add("mac");
  if (glassEnabled()) {
    root.classList.add("glass");
  } else {
    // Native glass view is created visible; hide it to honour the stored off.
    void invoke("set_glass_enabled", { enabled: false }).catch(() => {});
  }
}

const POLL_MS = 30 * 60 * 1000; // refresh every 30 min, like the Swift app

type Source = "live" | "local" | "team";
type WindowKey = "day" | "week" | "month";
type Provider = "claude" | "codex";
type View = "main" | "settings";

const PROVIDER_KEY = "hpbar-provider";
function loadProvider(): Provider {
  return localStorage.getItem(PROVIDER_KEY) === "codex" ? "codex" : "claude";
}

// Pinned popover: stays up on focus loss, floating like a desktop widget (the
// glassy "blurred" style is its unfocused look). The backend owns the actual
// don't-hide-on-blur behaviour; we re-sync it on launch below.
const PIN_KEY = "hpbar-pinned";

// The update channel persists like the provider — a single small string.
const CHANNEL_KEY = "hpbar-channel";
function loadChannel(): UpdateChannel {
  const v = localStorage.getItem(CHANNEL_KEY);
  return v === "beta" || v === "alpha" ? v : "stable";
}
function saveChannel(c: UpdateChannel): void {
  try {
    localStorage.setItem(CHANNEL_KEY, c);
  } catch {
    /* best-effort */
  }
}

// Remember the last-viewed tab/window/tool across launches so the popover opens
// where you left it (the Swift app always reset to Live, which was annoying when
// you live in Local or Team). Stored as one small JSON blob; invalid/old values
// fall back to the defaults below. The Team source is only restored once we've
// confirmed it's still enabled (see loadTeamConfig).
const VIEW_KEY = "hpbar-view";
interface SavedView {
  source?: Source;
  window?: WindowKey;
  localTool?: string;
  teamRange?: WindowKey;
}
function loadView(): SavedView {
  try {
    const v = JSON.parse(localStorage.getItem(VIEW_KEY) ?? "{}");
    return v && typeof v === "object" ? (v as SavedView) : {};
  } catch {
    return {};
  }
}
function saveView(): void {
  if (MOCK) return;
  try {
    const v: SavedView = {
      source: state.source,
      window: state.window,
      localTool: state.localTool,
      teamRange: state.teamRange,
    };
    localStorage.setItem(VIEW_KEY, JSON.stringify(v));
  } catch {
    /* localStorage may be unavailable; persistence is best-effort */
  }
}
const isWindowKey = (v: unknown): v is WindowKey => v === "day" || v === "week" || v === "month";
const isSource = (v: unknown): v is Source => v === "live" || v === "local" || v === "team";

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
  projectsExpanded: boolean; // Local "Top projects": show all vs the top few
  showDetail: boolean; // Live: reveal the device-share text + account email/plan
  pinned: boolean; // keep the popover up on focus loss (desktop-widget mode)
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
  teamCrimeMode: boolean;
  teamAccountKey: string; // "all" or a stable account key for bill splitting
  teamExpanded: Set<string>; // member ids whose top-projects are expanded
  teamDraft: TeamConfig | null; // edit buffer for the settings form
  settingsTab: SettingsTab; // which sub-tab of the Team form is showing
  settingsSection: SettingsSection; // top-level settings section (Update/Team/About)
  teamTesting: boolean;
  teamStatus: string;
  teamStatusOk: boolean;
  // General (relocated tray-menu controls; macOS only) + Update / About
  appControls: AppControls | null; // null until loaded from the backend
  appVersion: string; // running build's version, from the Rust side
  updateChannel: UpdateChannel;
  updateInfo: UpdateInfo | null;
  updateChecking: boolean;
  updateDownloading: boolean;
  updateStatus: string;
  updateStatusOk: boolean;
}

const savedView = loadView();
const state: State = {
  provider: loadProvider(),
  source: isSource(savedView.source) ? savedView.source : "live",
  view: "main",
  window: isWindowKey(savedView.window) ? savedView.window : "day",
  localTool: typeof savedView.localTool === "string" ? savedView.localTool : "all",
  selectedModelId: null,
  dropdownOpen: false,
  projectsExpanded: false,
  showDetail: false,
  pinned: localStorage.getItem(PIN_KEY) === "1",
  live: null,
  local: null,
  account: null,
  error: "",
  loading: false,
  updatedAt: "",
  teamConfig: null,
  team: null,
  teamRange: isWindowKey(savedView.teamRange) ? savedView.teamRange : "day",
  teamModel: "all",
  teamDropdownOpen: false,
  teamCrimeMode: params.get("crime") === "1",
  teamAccountKey: "all",
  teamExpanded: new Set(),
  teamDraft: null,
  settingsTab: "ssh",
  settingsSection: "update",
  teamTesting: false,
  teamStatus: "",
  teamStatusOk: false,
  appControls: null,
  appVersion: "",
  updateChannel: loadChannel(),
  updateInfo: null,
  updateChecking: false,
  updateDownloading: false,
  updateStatus: "",
  updateStatusOk: false,
};

// Re-arm the backend's don't-hide-on-blur behaviour from the stored preference
// (the Rust flag resets to false on every launch).
if (!MOCK && state.pinned) {
  invoke("set_pinned", { pinned: true }).catch(() => {});
}

const app = document.getElementById("app")!;
if (!MOCK && typeof ResizeObserver !== "undefined") {
  // Provider switches, async fonts and expanded Team rows can all change the
  // natural panel height outside the exact render frame. Keep the native
  // popover coupled to the content instead of leaving a fixed empty tail.
  new ResizeObserver(() => scheduleWindowSize()).observe(app);
}

// Apply showcase URL overrides.
const themeParam = params.get("theme");
if (isTheme(themeParam)) setThemeOverride(themeParam);
const sourceParam = params.get("source");
if (sourceParam === "live" || sourceParam === "local" || sourceParam === "team")
  state.source = sourceParam;
const providerParam = params.get("provider");
if (providerParam === "claude" || providerParam === "codex") state.provider = providerParam;
if (params.get("detail") === "1") state.showDetail = true; // showcase: pre-expand detail
if (params.get("expand") === "1") state.projectsExpanded = true; // showcase: full project list
const PACE_SHOWCASE = params.get("pace") === "1"; // showcase: force an over-pace 5-Hour
const CELEBRATE_SHOWCASE = params.get("celebrate") === "1"; // showcase: loop the refill animation
const HURT_SHOWCASE = params.get("hurt") === "1"; // showcase: loop the HP-drop animation

// ---------------------------------------------------------------- data

// A refresh captures the selected source/provider. If the user switches while
// an earlier request is still running, only the newest generation may update
// the UI; the older result is deliberately discarded.
let refreshGeneration = 0;

// Manual/auto team uploads are rate-limited so repeated refreshes (or range
// switches) can't spam SSH tunnels / the DB: at most one in flight, and no more
// than once per minute. The 30-min background uploader is separate/unaffected.
let teamUploadInFlight: Promise<string | null> | null = null;
let teamFetchInFlight: Promise<TeamReport> | null = null;
let teamFetchRange: WindowKey | null = null;
let lastTeamUploadAt = 0;
const TEAM_UPLOAD_MIN_MS = 60_000;
// Includes the local 90-day log scan that happens before the DB's own 60s
// deadline. Upload is background-only on Team, so a generous coalescing window
// is preferable to starting a duplicate scan while the first is still useful.
const TEAM_UPLOAD_DEADLINE_MS = 120_000;
const TEAM_FETCH_DEADLINE_MS = 35_000;

function withDeadline<T>(promise: Promise<T>, ms: number, message: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const deadline = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(message)), ms);
  });
  return Promise.race([promise, deadline]).finally(() => {
    if (timer !== undefined) clearTimeout(timer);
  });
}

/** Start a best-effort Team upload. Concurrent refreshes share one promise;
 * callers that only want to prime the next report can ignore its error. */
function maybeUploadTeam(force = false): Promise<string | null> {
  if (MOCK || !state.teamConfig?.enabled) return Promise.resolve(null);
  if (teamUploadInFlight) return teamUploadInFlight;
  const now = Date.now();
  if (!force && now - lastTeamUploadAt < TEAM_UPLOAD_MIN_MS) return Promise.resolve(null);
  lastTeamUploadAt = now;
  const upload = withDeadline(
    invoke("upload_team_snapshot"),
    TEAM_UPLOAD_DEADLINE_MS,
    "Team upload timed out after 120s.",
  )
    .then(() => null)
    .catch((err) => String(err))
    .finally(() => {
      if (teamUploadInFlight === upload) teamUploadInFlight = null;
    });
  teamUploadInFlight = upload;
  return upload;
}

/** Coalesce the startup/config-load double refresh onto one DB read. A range
 * change may start its own request, but every request still has a deadline. */
function fetchTeamReport(range: WindowKey): Promise<TeamReport> {
  if (teamFetchInFlight && teamFetchRange === range) return teamFetchInFlight;
  const request = withDeadline(
    invoke<TeamReport>("fetch_team", { range }),
    TEAM_FETCH_DEADLINE_MS,
    "Team refresh timed out after 35s.",
  ).finally(() => {
    if (teamFetchInFlight === request) {
      teamFetchInFlight = null;
      teamFetchRange = null;
    }
  });
  teamFetchInFlight = request;
  teamFetchRange = range;
  return request;
}

async function refresh(): Promise<void> {
  if (MOCK) {
    state.live = state.provider === "codex" ? mockCodexLive() : mockLive();
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
    if (CELEBRATE_SHOWCASE) {
      // Make the windows look freshly reset so the refill grows to ~full.
      state.live.windows.forEach((w) => {
        if (w.trailing !== "Off") {
          w.remaining = w.title === "Weekly" ? 0.93 : 0.97;
          w.utilization = 1 - w.remaining;
        }
      });
    }
    state.local = MOCK_LOCAL;
    state.teamConfig = MOCK_TEAM_CONFIG;
    state.team = MOCK_TEAM;
    state.account =
      state.provider === "codex"
        ? { email: "codex-team@example.com", plan: "Pro Lite" }
        : { email: "you@example.com", plan: "Max 20×" };
    state.selectedModelId = state.selectedModelId ?? MOCK_LOCAL.combined[0].id;
    state.error = "";
    state.updatedAt = nowTime();
    state.loading = false;
    render();
    return;
  }
  // Don't interrupt a refill celebration with a background poll / re-open — the
  // data won't meaningfully change in those ~7s, and a re-render would restart
  // the animation. The next poll after it ends picks up fresh data.
  if (celebrating.size) return;
  const generation = ++refreshGeneration;
  const provider = state.provider;
  const source = state.source;
  const teamRange = state.teamRange;
  const windowSecs = WINDOW_SECS[state.window];
  // Keep the last good Team report stable during a background refresh. The
  // header does not need an endless spinner while usable cached data exists.
  state.loading = source !== "team" || state.team === null;
  render();
  const codex = provider === "codex";
  // Refetch identity on every poll: it's a cheap local-file read, and the plan
  // label must track plan changes (the ~/.claude.json profile refreshes every
  // time Claude Code runs). Re-render only when it actually changed.
  invoke<Account>(codex ? "fetch_codex_account" : "fetch_account")
    .then((a) => {
      if (
        generation === refreshGeneration &&
        provider === state.provider &&
        (a.email !== state.account?.email || a.plan !== state.account?.plan)
      ) {
        state.account = a;
        render();
      }
    })
    .catch(() => {});
  try {
    if (source === "live") {
      // Subscription axis: the selected provider's quota.
      const live = await invoke<UsageReport>(codex ? "fetch_codex_quota" : "fetch_usage");
      if (generation !== refreshGeneration) return;
      state.live = live;
      // Queue a celebration for any window that reset since we last saw it, and
      // flash any window that dropped (painted by the final render below). The
      // celebration itself only plays while the popover is visible — this poll
      // also runs in the hidden webview, where an animation would play unseen.
      detectRefills(state.live).forEach((k) => pendingCelebration.add(k));
      savePendingCelebration();
      armDamage(detectDamage(state.live));
    } else if (source === "team") {
      // Uploading a 90-day snapshot can take longer than reading the roster,
      // especially over a high-latency SSH tunnel. It is best-effort and must
      // never hold the Team tab hostage; the next refresh picks up the result.
      void maybeUploadTeam();
      const team = await fetchTeamReport(teamRange);
      if (generation !== refreshGeneration) return;
      state.team = team;
      // Keep the model selection valid as the range/data changes.
      if (state.teamModel !== "all" && !state.team.models.some((m) => m.id === state.teamModel)) {
        state.teamModel = "all";
      }
      if (
        state.teamAccountKey !== "all" &&
        !state.team.accounts.some((a) => a.account_key === state.teamAccountKey)
      ) {
        state.teamAccountKey = "all";
      }
    } else {
      // API axis: every local tool at once (not provider-scoped).
      const local = await invoke<LocalReport>("fetch_local", { windowSecs });
      if (generation !== refreshGeneration) return;
      state.local = local;
      snapSelectedModel();
    }
    state.error = "";
    state.updatedAt = nowTime();
  } catch (err) {
    if (generation === refreshGeneration) state.error = String(err);
  } finally {
    if (generation === refreshGeneration) {
      state.loading = false;
      render();
    }
  }
  if (generation === refreshGeneration) void maybePlayCelebrations();
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
let windowSizeGeneration = 0;
let lastRequestedHeight = 0;

function render(): void {
  app.innerHTML =
    state.view === "settings"
      ? `<main class="panel">${settingsHeaderHTML()}${settingsSectionSegHTML()}<section class="content">${settingsBodyHTML()}</section></main>`
      : `<main class="panel">
      ${headerHTML()}
      ${sourceSegHTML()}
      ${filterLineHTML()}
      <section class="content">${contentHTML()}</section>
      ${footerHTML()}
    </main>`;
  scheduleWindowSize();
}

// The slim "filter line" under the source tabs holds each view's secondary
// controls, kept compact so the tabs stay the only chunky row. Often-toggled
// ranges are visible segments; rarely-changed tool/provider use dropdowns.
function filterLineHTML(): string {
  // Live has no filter row — the provider toggle lives in the title (headerHTML).
  if (state.source === "local") return localFilterHTML();
  if (state.source === "team") return teamFilterHTML();
  return "";
}

// Local: the tool as a click-to-cycle chip (advances All → Claude Code → … on
// click) on the left; the window range (segment — toggled often) on the right.
function localFilterHTML(): string {
  const apps = state.local?.apps ?? [];
  const opts = [{ id: "all", name: "All tools" }, ...apps.map((a) => ({ id: a.id, name: a.display_name }))];
  const current = opts.find((o) => o.id === state.localTool) ?? opts[0];
  const chip = `<button class="mc-btn tool-cycle" data-action="tool-cycle" title="Switch tool">${escapeHTML(
    current.name,
  )} <span class="title-swap">⇄</span></button>`;
  const windowSeg = `<div class="seg">${(["day", "week", "month"] as WindowKey[])
    .map((w) => segButton("window", w, WINDOW_TITLE[w], state.window === w))
    .join("")}</div>`;
  return `<div class="filter-line">${chip}${windowSeg}</div>`;
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
      identity_version: 2,
      legacy_member_id: "",
      display_name: "",
      share_tokens: true,
      share_cost: true,
      share_project: true,
      share_account: false,
      account_label_mode: "masked",
      interval_secs: 1800,
      backfill_days: 90,
      top_projects: 5,
    }
  );
}

/** Measure after two layout frames: the first commits the new provider view,
 * the second catches font/bar geometry. A generation guard prevents an older
 * Claude measurement from winning after the user switches to shorter Codex. */
function scheduleWindowSize(): void {
  if (MOCK) return;
  const generation = ++windowSizeGeneration;
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      void syncWindowSize(generation);
    }),
  );
}

async function syncWindowSize(generation: number): Promise<void> {
  if (generation !== windowSizeGeneration) return;
  const panel = app.querySelector(".panel") as HTMLElement | null;
  if (!panel) return;
  const h = Math.ceil(panel.getBoundingClientRect().height);
  if (h <= 1) return;
  if (h === lastRequestedHeight) return;
  lastRequestedHeight = h;
  try {
    await getCurrentWindow().setSize(new LogicalSize(PANEL_WIDTH, h));
  } catch {
    // A later render/ResizeObserver notification gets another chance.
    if (generation === windowSizeGeneration) lastRequestedHeight = 0;
  }
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
  // On Live the title doubles as the provider switch: click to flip Claude⇄Codex
  // (it already names the provider), so Live needs no separate control row.
  const titleHTML =
    state.source === "live"
      ? `<span class="title title-switch" data-action="provider-cycle" title="Switch to ${
          state.provider === "codex" ? "Claude" : "Codex"
        }">${escapeHTML(title)} <span class="title-swap">⇄</span></span>`
      : `<span class="title">${escapeHTML(title)}</span>`;
  return `
    <header class="header">
      ${titleHTML}
      ${state.loading ? `<span class="spinner">…</span>` : ""}
      <button class="mc-btn icon ${state.pinned ? "selected" : ""}" data-action="pin" title="${
        state.pinned ? "Unpin" : "Pin as floating widget"
      }">${pinIcon(getTheme())}</button>
      <button class="mc-btn icon" data-action="theme" title="Theme">${themeLabel(getTheme())}</button>
      <button class="mc-btn icon" data-action="settings" title="Settings">${settingsIcon(getTheme())}</button>
      <button class="mc-btn icon" data-action="refresh" title="Refresh">${refreshIcon(getTheme())}</button>
    </header>`;
}

function settingsHeaderHTML(): string {
  return `
    <header class="header">
      <span class="title">Settings</span>
      <button class="mc-btn icon" data-action="theme" title="Theme">${themeLabel(getTheme())}</button>
      <button class="mc-btn icon" data-action="settings-close" title="Back">✕</button>
    </header>`;
}

// Top-level settings sections. General (app controls + Quit) leads on macOS,
// where it replaces the tray menu; then Update (universally useful), Team
// (opt-in), About. General is macOS-only — elsewhere the tray menu still owns
// those toggles, so showing them here too would desync the menu's checkmarks.
const SETTINGS_SECTIONS: { id: SettingsSection; label: string }[] = [
  ...(IS_MAC ? [{ id: "general" as const, label: "General" }] : []),
  { id: "update", label: "Update" },
  { id: "team", label: "Team" },
  { id: "about", label: "About" },
];

function settingsSectionSegHTML(): string {
  const seg = SETTINGS_SECTIONS.map(
    (s) =>
      `<button class="mc-btn ${s.id === state.settingsSection ? "selected" : ""}" data-action="settings-section" data-value="${s.id}">${s.label}</button>`,
  ).join("");
  return `<div class="seg">${seg}</div>`;
}

// Render the active section's body. Team reuses the existing form (with its own
// SSH/DB/Team sub-tabs); Update and About come from about.ts.
function settingsBodyHTML(): string {
  if (state.settingsSection === "general") {
    return generalSectionHTML({
      controls: state.appControls,
      // Liquid Glass toggle is macOS-only.
      glass: IS_MAC ? glassEnabled() : undefined,
    });
  }
  if (state.settingsSection === "team") {
    return settingsContentHTML({
      draft: state.teamDraft ?? defaultTeamDraft(),
      tab: state.settingsTab,
      status: state.teamStatus,
      statusOk: state.teamStatusOk,
      testing: state.teamTesting,
    });
  }
  if (state.settingsSection === "about") {
    return aboutSectionHTML({ version: state.appVersion });
  }
  return updateSectionHTML({
    channel: state.updateChannel,
    info: state.updateInfo,
    status: state.updateStatus,
    statusOk: state.updateStatusOk,
    checking: state.updateChecking,
    downloading: state.updateDownloading,
    version: state.appVersion,
  });
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
      crimeMode: state.teamCrimeMode,
      accountKey: state.teamAccountKey,
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
  if (state.live)
    return state.live.windows.map(liveBarHTML).join("") + usageDetailsHTML(state.live);
  if (state.error) return `<div class="msg">${escapeHTML(state.error)}</div>`;
  return `<div class="msg">Loading…</div>`;
}

function usageDetailsHTML(report: UsageReport): string {
  if (report.details.length === 0) return "";
  const rows = report.details
    .map(
      (d) =>
        `<div class="quota-detail"><span>${escapeHTML(d.label)}</span><span>${escapeHTML(
          d.value,
        )}</span></div>`,
    )
    .join("");
  return `<div class="quota-details">${rows}</div>`;
}

// --- Refill celebration -----------------------------------------------------
// When a window you'd actually used (≥ REFILL_MIN_PRIOR utilization) resets,
// play a ~7s per-theme "refill" animation, once per reset. The last-seen
// utilization + reset clock are persisted per window, so a reset that happened
// while the popover was closed (or while the app wasn't running) still
// celebrates — and because the hidden webview keeps polling, detected refills
// wait in `pendingCelebration` until the popover is actually on screen.
const CELEBRATE_MS = 7_300; // a hair past the 7s CSS animation, then settle
const REFILL_MIN_PRIOR = 0.5; // must have used ≥ half to "earn" the cheer
const REFILL_MIN_DROP = 0.3; // utilization must actually *fall* this much (a real reset)
const REFILL_KEY = "hpbar-refill-v2";
const REFILL_KEY_V1 = "hpbar-refill"; // pre-0.8.1: bare utilization numbers
// v2 (0.8.1) queued false positives; the rename orphans any stored junk.
const REFILL_PENDING_KEY = "hpbar-refill-pending-v3";
const REFILL_PENDING_KEY_V2 = "hpbar-refill-pending";

// Per-window baseline: utilization at the last poll. (0.8.1 also stored the
// reset clock `r`; loading tolerates and drops it.)
type RefillSeen = Record<string, { u: number }>;

const celebrating = new Set<string>(); // window keys currently animating
const pendingCelebration = new Set<string>(); // detected, waiting to be visible
let celebrateTimer: ReturnType<typeof setTimeout> | undefined;

const winKey = (w: UsageWindow): string => `${state.provider}/${w.title}`;

function loadRefillState(): RefillSeen {
  try {
    const v2 = JSON.parse(localStorage.getItem(REFILL_KEY) ?? "null");
    if (v2 && typeof v2 === "object") {
      return Object.fromEntries(
        Object.entries(v2 as Record<string, { u: number }>).map(([k, v]) => [k, { u: Number(v.u) }]),
      );
    }
    const v1 = JSON.parse(localStorage.getItem(REFILL_KEY_V1) ?? "null");
    if (v1 && typeof v1 === "object") {
      return Object.fromEntries(
        Object.entries(v1 as Record<string, number>).map(([k, u]) => [k, { u: Number(u) }]),
      );
    }
  } catch {
    /* fall through */
  }
  return {};
}
function saveRefillState(s: RefillSeen): void {
  try {
    localStorage.setItem(REFILL_KEY, JSON.stringify(s));
  } catch {
    /* best-effort */
  }
}
function savePendingCelebration(): void {
  try {
    localStorage.setItem(REFILL_PENDING_KEY, JSON.stringify([...pendingCelebration]));
  } catch {
    /* best-effort */
  }
}
function loadPendingCelebration(): void {
  try {
    localStorage.removeItem(REFILL_PENDING_KEY_V2); // may hold 0.8.1 false positives
    const v = JSON.parse(localStorage.getItem(REFILL_PENDING_KEY) ?? "[]");
    if (Array.isArray(v)) v.forEach((k) => typeof k === "string" && pendingCelebration.add(k));
  } catch {
    /* best-effort */
  }
}

// Compare live windows against the persisted baseline; return the keys whose
// window actually reset since we last looked, and update the baseline. Within
// a window utilization only ever climbs, so the one trustworthy reset signal
// is utilization *falling* — and by a margin (REFILL_MIN_DROP) that neither
// rounding jitter nor a rolling window slowly aging out old usage can produce.
// This still catches a poll landing late into an already-in-use fresh window.
// (Watching `resets_at` move was tried in 0.8.1 and fires falsely: some
// sources recompute it on every fetch.)
function detectRefills(report: UsageReport): string[] {
  const seen = loadRefillState();
  const refilled: string[] = [];
  for (const w of report.windows) {
    if (w.trailing === "Off") continue;
    const key = winKey(w);
    const prev = seen[key];
    if (prev != null && prev.u >= REFILL_MIN_PRIOR && prev.u - w.utilization >= REFILL_MIN_DROP) {
      refilled.push(key);
    }
    seen[key] = { u: w.utilization };
  }
  saveRefillState(seen);
  return refilled;
}

// Play the queued celebrations — but only when the popover is actually on
// screen. The webview stays alive (and polling) while hidden, and an animation
// played to a hidden window is one the user never sees; pending keys survive
// until a poll or popover-open finds the window visible.
async function maybePlayCelebrations(): Promise<void> {
  if (!pendingCelebration.size || celebrating.size) return;
  // Only the Live view renders the bars; keep the queue until it's on screen.
  if (state.view === "settings" || state.source !== "live") return;
  let visible = true;
  try {
    visible = await getCurrentWindow().isVisible();
  } catch {
    /* browser/mock mode: treat as visible */
  }
  if (!visible) return;
  const keys = [...pendingCelebration];
  pendingCelebration.clear();
  savePendingCelebration();
  armCelebration(keys);
  render();
}

// Mark windows as celebrating + schedule the clear. Does not render (the caller
// renders) — callers arm it and let their own render paint the animation.
function armCelebration(keys: string[]): void {
  if (!keys.length) return;
  keys.forEach((k) => celebrating.add(k));
  if (celebrateTimer) clearTimeout(celebrateTimer);
  celebrateTimer = setTimeout(() => {
    celebrating.clear();
    render();
  }, CELEBRATE_MS);
}
// Showcase only: loop the refill animation over all resettable live windows so
// the `?celebrate=1` mock page plays it continuously (one panel per theme).
function runCelebrationShowcase(): void {
  const keys = (state.live?.windows ?? [])
    .filter((w) => w.resets_at && w.trailing !== "Off")
    .map(winKey);
  if (!keys.length) return;
  const loop = () => {
    celebrating.clear();
    keys.forEach((k) => celebrating.add(k));
    render();
    celebrateTimer = setTimeout(() => {
      celebrating.clear();
      render();
      setTimeout(loop, 1800); // brief settled pause, then replay
    }, CELEBRATE_MS);
  };
  loop();
}

// Showcase only: loop the HP-drop animation over the live windows so the
// `?hurt=1` mock page plays it continuously. Each cycle drops every window a
// notch (animating from its base level), then restores and replays.
function runHurtShowcase(): void {
  const windows = (state.live?.windows ?? []).filter((w) => w.trailing !== "Off");
  if (!windows.length) return;
  const base = windows.map((w) => clamp01(w.remaining));
  const loop = () => {
    windows.forEach((w, i) => {
      hurting.set(winKey(w), base[i]);
      const to = Math.max(0.05, base[i] - 0.14);
      w.remaining = to;
      w.utilization = 1 - to;
    });
    render();
    hurtTimer = setTimeout(() => {
      hurting.clear();
      windows.forEach((w, i) => {
        w.remaining = base[i];
        w.utilization = 1 - base[i];
      });
      render();
      setTimeout(loop, 1400); // brief settled pause, then replay
    }, HURT_MS);
  };
  loop();
}

// User interaction (or navigating away) ends the show so the app stays responsive.
function cancelCelebration(): void {
  if (!celebrating.size) return;
  celebrating.clear();
  if (celebrateTimer) clearTimeout(celebrateTimer);
}

// --- Damage (HP drop) animation ---------------------------------------------
// The mirror of the refill cheer: when a live window's remaining fraction *falls*
// between polls, flash the bar once (Classic/Arknights shrink smoothly from the
// old level; Minecraft shakes the heart row). Tracked in-memory only — a drop
// that happened while the popover was closed shouldn't ambush you on reopen, and
// a first load (no baseline yet) never animates.
const HURT_MS = 850; // a hair past the ~0.7s CSS animation
const HURT_MIN_DROP = 0.02; // ignore sub-2% jitter / rounding wobble
const lastRemaining = new Map<string, number>(); // window key → remaining at last poll
const hurting = new Map<string, number>(); // window key → remaining *before* the drop (animate from)
let hurtTimer: ReturnType<typeof setTimeout> | undefined;

// Compare this report to the last poll; return the keys that dropped mapped to
// their previous (higher) remaining, and update the baseline.
function detectDamage(report: UsageReport): Map<string, number> {
  const dropped = new Map<string, number>();
  for (const w of report.windows) {
    const key = winKey(w);
    const prev = lastRemaining.get(key);
    lastRemaining.set(key, w.remaining);
    if (w.trailing === "Off") continue; // disabled window has no meaningful level
    if (prev != null && prev - w.remaining >= HURT_MIN_DROP) {
      dropped.set(key, clamp01(prev));
    }
  }
  return dropped;
}

// Arm the hurt flash for the dropped windows + schedule the clear. Like
// `armCelebration`, it doesn't render — the caller's final render paints it.
function armDamage(dropped: Map<string, number>): void {
  if (!dropped.size) return;
  dropped.forEach((from, k) => hurting.set(k, from));
  if (hurtTimer) clearTimeout(hurtTimer);
  hurtTimer = setTimeout(() => {
    hurting.clear();
    render();
  }, HURT_MS);
}

// The animated stand-in for a live bar while its window is celebrating a refill.
function celebrationWindowHTML(w: UsageWindow): string {
  const r = clamp01(w.remaining);
  let bar: string;
  switch (getTheme()) {
    case "classic":
      bar = classicRefillBar(w.title, r);
      break;
    case "arknights":
      bar = akRefillBar(w.title, r);
      break;
    default:
      bar = `
        <div class="bar">
          <div class="bar-head">
            <span class="bar-title">${escapeHTML(w.title)}</span>
            <span class="bar-trailing">refilled ♥</span>
          </div>
          <div class="hearts">${heartsRefillHTML(r)}</div>
        </div>`;
  }
  return `<div class="live-window">${bar}</div>`;
}

function liveBarHTML(w: UsageWindow): string {
  if (celebrating.has(winKey(w))) return celebrationWindowHTML(w);
  const used = Math.round(w.utilization * 100);
  const left = Math.round(w.remaining * 100);
  const trailing = w.trailing ?? `${used}% used · ${left}% left`;
  const reset = resetCaption(w.resets_at);
  const pace = paceNote(w); // "20% over pace" on the 5-Hour bar when burning fast
  const resetText = [reset, pace].filter(Boolean).join(" · ");
  const split = machineSplit(w); // this machine's window fraction, or null when unsure
  const hurtFrom = hurting.get(winKey(w)) ?? null; // pre-drop level to animate down from
  let bar: string;
  switch (getTheme()) {
    // Caption is rendered below (in `.live-foot`), not by the theme bar, so the
    // share text can ride its right edge — pass null caption to each renderer.
    case "classic":
      bar = classicQuotaBar(w.title, w.remaining, trailing, null, split, w.trailing === "Off", hurtFrom);
      break;
    case "arknights":
      bar = akResource(w.title, w.remaining, null, w.trailing, split, hurtFrom);
      break;
    default:
      bar = heartBarHTML(w, trailing, null, split, hurtFrom);
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
  hurtFrom: number | null = null,
): string {
  const hearts =
    machineShare != null ? heartsRowSplit(w.remaining, machineShare) : heartsRow(w.remaining);
  // On a drop, shake+flash the heart row (Minecraft damage). The hearts already
  // show the new, lower level; the class just animates the hit.
  const hurt = hurtFrom != null ? " hp-hurt" : "";
  return `
    <div class="bar">
      <div class="bar-head">
        <span class="bar-title">${escapeHTML(w.title)}</span>
        <span class="bar-trailing">${escapeHTML(trailing)}</span>
      </div>
      <div class="hearts${hurt}">${hearts}</div>
      ${caption ? `<div class="bar-caption">${escapeHTML(caption)}</div>` : ""}
    </div>`;
}

// Even-pace check: use the provider-reported duration, so a weekly-only Codex
// plan remains useful and a future window shape needs no frontend change.
const PACE_THRESHOLD = 0.12; // only flag a meaningful lead (12 percentage points)

// "20% over pace" when you've burned notably more than an even spend-down would
// predict by now — a gentle nudge to slow before you hit the harder eta warning.
// null when on/under pace, unknown duration, or the eta warning is already shown.
function paceNote(w: UsageWindow): string | null {
  if (w.eta_secs != null) return null; // the stronger "hits limit" warning wins
  const dur = w.window_minutes ? w.window_minutes * 60 : 0;
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
  // When expanded, the rows sit in a capped, internally-scrolling box (so "Show
  // less" stays on-screen) wrapped by `.proj-scroll-wrap`, which paints the soft
  // bottom fade hinting there's more. The browser scrollbar itself is hidden (CSS).
  const rowsDiv = `<div class="xp-bars proj-rows${state.projectsExpanded ? " scroll" : ""}">${rows}</div>`;
  const body = state.projectsExpanded ? `<div class="proj-scroll-wrap">${rowsDiv}</div>` : rowsDiv;
  return `<div class="proj-section"><div class="proj-title">Top projects</div>${body}${toggle}</div>`;
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
      ${m.unattributed > 0 ? bar("Other", frac(m.unattributed), formatTokens(m.unattributed)) : ""}
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
  // "detail" toggle), member count on Team. When detail is off, omit the line
  // entirely and use the compact one-row footer; content-driven window sizing
  // will grow it again when detail is opened.
  let midLine = "";
  if (state.source === "live") {
    // The per-account subscription (Claude login, or the ChatGPT login behind
    // Codex), shown only when "detail" is on. Hidden on Local, which aggregates
    // models across accounts.
    const acctText =
      state.showDetail && state.account
        ? [state.account.email, state.account.plan].filter(Boolean).join(" · ")
        : "";
    if (acctText) midLine = `<div class="account">${escapeHTML(acctText)}</div>`;
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
  const compact = state.source === "live" && !midLine;
  return `
    <footer class="footer ${compact ? "footer-compact" : ""}">
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
  cancelCelebration(); // a click ends the refill show; the action re-renders below

  switch (action) {
    case "refresh":
      // An explicit click is the one moment we're allowed to look at Claude
      // Code's credential storage again even if it appears unchanged — a
      // background poll must never do that (it can raise a Keychain prompt).
      // No-op when the cached token is healthy.
      if (MOCK || state.source !== "live" || state.provider !== "claude") {
        void refresh();
      } else {
        void invoke("recheck_credentials")
          .catch(() => {})
          .then(() => refresh());
      }
      break;
    case "provider-cycle":
      // Title click on Live: flip Claude⇄Codex (only two providers).
      state.provider = state.provider === "codex" ? "claude" : "codex";
      localStorage.setItem(PROVIDER_KEY, state.provider);
      state.live = null; // provider drives the Live quota + its account footer
      state.account = null;
      state.error = "";
      render();
      void refresh();
      break;
    case "tool-cycle": {
      // Tool chip click: advance to the next tool (wraps), no refetch.
      const ids = ["all", ...(state.local?.apps ?? []).map((a) => a.id)];
      const i = ids.indexOf(state.localTool);
      state.localTool = ids[(i + 1) % ids.length] ?? "all";
      state.dropdownOpen = false;
      snapSelectedModel(); // re-render from already-loaded data
      saveView();
      render();
      break;
    }
    case "theme":
      cycleTheme();
      syncTrayTheme(); // recolor the menu-bar heart to match
      render();
      break;
    case "pin":
      state.pinned = !state.pinned;
      try {
        localStorage.setItem(PIN_KEY, state.pinned ? "1" : "0");
      } catch {
        /* best-effort */
      }
      if (!MOCK) void invoke("set_pinned", { pinned: state.pinned }).catch(() => {});
      render();
      break;
    case "source":
      if (value && value !== state.source) {
        state.source = value as Source;
        state.error = "";
        state.dropdownOpen = false;
        saveView();
        render(); // show cached view instantly…
        void refresh(); // …then refresh in the background
      }
      break;
    case "window":
      if (value && value !== state.window) {
        state.window = value as WindowKey;
        saveView();
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
        saveView();
        render();
        void refresh();
      }
      break;
    case "team-toggle-dropdown":
      state.teamDropdownOpen = !state.teamDropdownOpen;
      render();
      break;
    case "team-crime-toggle":
      state.teamCrimeMode = !state.teamCrimeMode;
      state.teamDropdownOpen = false;
      render();
      break;
    case "team-account-cycle": {
      const ids = ["all", ...(state.team?.accounts ?? []).map((a) => a.account_key)];
      const i = ids.indexOf(state.teamAccountKey);
      state.teamAccountKey = ids[(i + 1) % ids.length] ?? "all";
      render();
      break;
    }
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
    case "settings-tab":
      if (value) state.settingsTab = value as SettingsTab;
      render();
      break;
    case "settings-section":
      if (value && value !== state.settingsSection) {
        state.settingsSection = value as SettingsSection;
        render();
        // Auto-check the first time Update is opened with nothing cached.
        if (state.settingsSection === "update" && !state.updateInfo) void checkUpdate();
      }
      break;
    case "update-channel":
      if (value && value !== state.updateChannel) {
        state.updateChannel = value as UpdateChannel;
        saveChannel(state.updateChannel);
        state.updateInfo = null; // result no longer matches the channel
        render();
        void checkUpdate();
      }
      break;
    case "update-check":
      void checkUpdate();
      break;
    case "update-install":
      void installUpdate();
      break;
    case "open-url":
      if (value) void invoke("open_external", { target: value }).catch(() => {});
      break;
    case "quit-app":
      // The only Quit affordance on macOS now that the tray has no menu.
      if (!MOCK) void invoke("quit_app").catch(() => {});
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
  if (!field) return;
  // General-section controls (macOS): route straight to the backend.
  if (field.startsWith("ctl-")) {
    void applyControl(field.slice(4) as keyof AppControls, el.checked);
    return;
  }
  // Liquid Glass toggle (macOS, frontend setting): flip the `glass` class and
  // show/hide the native backdrop.
  if (field === "glass") {
    const on = el.checked;
    try {
      localStorage.setItem(GLASS_KEY, on ? "1" : "0");
    } catch {
      /* best-effort */
    }
    document.documentElement.classList.toggle("glass", on);
    void invoke("set_glass_enabled", { enabled: on }).catch(() => {});
    return;
  }
  if (!state.teamDraft) return;
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
  state.settingsTab = "ssh";
  state.settingsSection = "update";
  state.teamStatus = "";
  state.teamStatusOk = false;
  state.teamTesting = false;
  state.updateStatus = "";
  state.view = "settings";
  render();
  void loadAppVersion();
  if (IS_MAC) void loadAppControls(); // General section (macOS)
  // Land on Update with a check already in flight, so there's nothing to click
  // for the common "is there a new version?" case.
  void checkUpdate();
}

// The relocated tray-menu toggles' current state (macOS). Loaded when settings
// opens so the General section reflects the real backend values.
async function loadAppControls(): Promise<void> {
  if (MOCK) {
    state.appControls = { autostart: false, alerts: true, calibrate: false };
    render();
    return;
  }
  try {
    state.appControls = await invoke<AppControls>("get_app_controls");
    render();
  } catch {
    /* leave null; toggles stay disabled */
  }
}

// Push one control change to the backend, reflecting it optimistically. On
// failure, re-read the real state so the toggle can't drift out of sync.
async function applyControl(key: keyof AppControls, on: boolean): Promise<void> {
  if (state.appControls) state.appControls[key] = on;
  if (MOCK) return;
  const cmd =
    key === "autostart" ? "set_autostart" : key === "alerts" ? "set_alerts_enabled" : "set_calibrate";
  try {
    await invoke(cmd, { enabled: on });
  } catch {
    void loadAppControls(); // reconcile with the backend's actual state
  }
}

// The running build's version (cheap, cached after the first call).
async function loadAppVersion(): Promise<void> {
  if (state.appVersion) return;
  if (MOCK) {
    state.appVersion = "0.0.0-mock";
    return;
  }
  try {
    state.appVersion = await invoke<string>("app_version");
    render();
  } catch {
    /* leave blank; the UI shows "?" */
  }
}

async function checkUpdate(): Promise<void> {
  if (MOCK || state.updateChecking) return;
  state.updateChecking = true;
  state.updateStatus = "";
  render();
  try {
    state.updateInfo = await invoke<UpdateInfo>("check_update", { channel: state.updateChannel });
    state.updateStatusOk = true;
    state.updateStatus = ""; // the result block conveys latest/running/count
  } catch (err) {
    state.updateInfo = null;
    state.updateStatusOk = false;
    state.updateStatus = String(err);
  } finally {
    state.updateChecking = false;
    render();
  }
}

async function installUpdate(): Promise<void> {
  const info = state.updateInfo;
  if (MOCK || !info || !info.has_asset || state.updateDownloading) return;
  state.updateDownloading = true;
  state.updateStatus = "";
  render();
  try {
    const path = await invoke<string>("download_and_install_update", {
      url: info.asset_url,
      name: info.asset_name,
    });
    state.updateStatusOk = true;
    state.updateStatus = `Opened the installer (saved to ${path}).`;
  } catch (err) {
    state.updateStatusOk = false;
    state.updateStatus = String(err);
  } finally {
    state.updateDownloading = false;
    render();
  }
}

async function testTeam(): Promise<void> {
  if (!state.teamDraft || state.teamTesting) return;
  state.teamTesting = true;
  state.teamStatus = "";
  render();
  try {
    const h = await invoke<TeamHandshake>("test_team_connection", { config: state.teamDraft });
    lastTeamUploadAt = Date.now();
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
    const previous = state.teamConfig;
    await invoke("set_team_config", { config: state.teamDraft });
    state.teamConfig = await invoke<TeamConfig>("get_team_config");
    const sharingChanged =
      !previous ||
      previous.share_tokens !== state.teamConfig.share_tokens ||
      previous.share_cost !== state.teamConfig.share_cost ||
      previous.share_project !== state.teamConfig.share_project ||
      previous.share_account !== state.teamConfig.share_account ||
      previous.account_label_mode !== state.teamConfig.account_label_mode;
    if (state.teamConfig.enabled && sharingChanged) {
      const uploadError = await maybeUploadTeam(true);
      if (uploadError) {
        state.teamStatusOk = false;
        state.teamStatus = `Saved locally, but the new sharing settings have not reached the Team DB: ${uploadError}`;
        render();
        return;
      }
    }
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
    // We may have restored source="team" from a previous session; if sharing is
    // no longer enabled, the Team tab won't render, so fall back to Live.
    if (state.source === "team" && !state.teamConfig.enabled) {
      state.source = "live";
      saveView();
      void refresh();
    } else if (state.source === "team") {
      // The initial refresh can run before this async config load, in which
      // case it cannot prime the v2 member. Repeat once with the config present.
      void refresh();
    }
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
loadPendingCelebration(); // refills detected but never shown before last quit
render();
void refresh();
void loadTeamConfig();
syncTrayTheme();
if (CELEBRATE_SHOWCASE) runCelebrationShowcase(); // mock page: loop the refill animation
if (HURT_SHOWCASE) runHurtShowcase(); // mock page: loop the HP-drop animation
// Showcase: `?view=settings[&section=general]` deep-links into the settings view
// (there's no click to simulate in a screenshot). Honors the section if valid.
if (params.get("view") === "settings") {
  openSettings();
  const sec = params.get("section");
  if (sec && SETTINGS_SECTIONS.some((s) => s.id === sec)) {
    state.settingsSection = sec as SettingsSection;
    render();
  }
}

// Tauri-only wiring: re-fetch when the popover opens, and on a slow timer.
// Skipped in showcase/browser mode (no Tauri runtime).
if (!MOCK) {
  listen<UsageReport>("claude-usage-updated", (event) => {
    if (state.provider !== "claude") return;
    state.live = event.payload;
    if (state.source === "live") {
      // A successful background response is newer than any foreground error
      // still in flight, so make it authoritative and clear the stale 403.
      refreshGeneration += 1;
      state.error = "";
      state.loading = false;
      state.updatedAt = nowTime();
      render();
    }
  });
  listen("refresh", () => {
    // Popover just became visible: play any refill detected while hidden, even
    // if the refresh itself short-circuits (poll already in flight, etc.).
    void refresh();
    void maybePlayCelebrations();
  });
  setInterval(() => void refresh(), POLL_MS);
}
