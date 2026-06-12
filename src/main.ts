import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/dpi";
import type { Account, LocalReport, ModelUsage, UsageReport, UsageWindow } from "./types";
import { heartsRow } from "./hearts";
import { xpBar } from "./xpbar";
import { clamp01, escapeHTML, formatDollars, formatTokens, nowTime } from "./util";
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

type Source = "live" | "local";
type WindowKey = "day" | "week" | "month";

const WINDOW_SECS: Record<WindowKey, number> = {
  day: 86_400,
  week: 604_800,
  month: 2_592_000,
};
const WINDOW_TITLE: Record<WindowKey, string> = { day: "24h", week: "7d", month: "30d" };

interface State {
  source: Source;
  window: WindowKey;
  selectedModelId: string | null;
  dropdownOpen: boolean;
  live: UsageReport | null;
  local: LocalReport | null;
  account: Account | null;
  error: string;
  loading: boolean;
  updatedAt: string;
}

const state: State = {
  source: "live",
  window: "day",
  selectedModelId: null,
  dropdownOpen: false,
  live: null,
  local: null,
  account: null,
  error: "",
  loading: false,
  updatedAt: "",
};

const app = document.getElementById("app")!;

// Apply showcase URL overrides.
const themeParam = params.get("theme");
if (isTheme(themeParam)) setThemeOverride(themeParam);
const sourceParam = params.get("source");
if (sourceParam === "live" || sourceParam === "local") state.source = sourceParam;

// ---------------------------------------------------------------- data

let inFlight = false;

async function refresh(): Promise<void> {
  if (MOCK) {
    state.live = mockLive();
    state.local = MOCK_LOCAL;
    state.account = { email: "you@example.com", plan: "Max 20×" };
    state.selectedModelId = state.selectedModelId ?? MOCK_LOCAL.models[0].id;
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
  // Account identity rarely changes; fetch it once and reuse.
  if (!state.account) {
    invoke<Account>("fetch_account")
      .then((a) => {
        state.account = a;
        render();
      })
      .catch(() => {});
  }
  try {
    if (state.source === "live") {
      state.live = await invoke<UsageReport>("fetch_usage");
    } else {
      const report = await invoke<LocalReport>("fetch_local", {
        windowSecs: WINDOW_SECS[state.window],
      });
      state.local = report;
      // Keep the selection valid: snap to the highest-volume model.
      if (!report.models.some((m) => m.id === state.selectedModelId)) {
        state.selectedModelId = report.models[0]?.id ?? null;
      }
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

// ---------------------------------------------------------------- render

const PANEL_WIDTH = 360;

function render(): void {
  app.innerHTML = `
    <main class="panel">
      ${headerHTML()}
      ${sourceSegHTML()}
      <section class="content">${contentHTML()}</section>
      ${footerHTML()}
    </main>`;
  // Resize the window to the content height (top-anchored → grows downward),
  // mirroring the AppKit panel's self-sizing.
  requestAnimationFrame(syncWindowSize);
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
  return `
    <header class="header">
      <span class="title">Claude Quota</span>
      ${state.loading ? `<span class="spinner">…</span>` : ""}
      <button class="mc-btn icon" data-action="theme" title="Theme">${themeLabel(getTheme())}</button>
      <button class="mc-btn icon" data-action="refresh" title="Refresh">⟳</button>
    </header>`;
}

function segButton(action: string, value: string, label: string, selected: boolean): string {
  return `<button class="mc-btn ${selected ? "selected" : ""}" data-action="${action}" data-value="${value}">${label}</button>`;
}

function sourceSegHTML(): string {
  return `
    <div class="seg">
      ${segButton("source", "live", "Live quota", state.source === "live")}
      ${segButton("source", "local", "Local activity", state.source === "local")}
    </div>`;
}

function contentHTML(): string {
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
  switch (getTheme()) {
    case "classic":
      return classicQuotaBar(w.title, w.remaining, trailing, caption);
    case "arknights":
      return akResource(w.title, w.remaining, caption, w.trailing);
    default:
      return heartBarHTML(w, trailing, caption);
  }
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

  const models = state.local.models;
  const current = models.find((m) => m.id === state.selectedModelId) ?? models[0];
  if (!current) {
    return windowSeg + `<div class="msg">No model activity in this window.</div>`;
  }

  return windowSeg + dropdownHTML(models, current) + costLineHTML(current) + xpBarsHTML(current);
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
    state.source === "live"
      ? (state.live?.source_label ?? "Live quota")
      : (state.local?.source_label ?? "Local activity");
  const updated = state.updatedAt ? `Updated ${state.updatedAt}` : "";
  // Account identity only makes sense for Live quota — that's Claude's official
  // per-account usage. Local activity aggregates every model in the transcripts
  // (incl. non-Claude providers like DeepSeek/Volcengine), so the Claude login
  // and plan are irrelevant there.
  const acct = state.source === "live" ? state.account : null;
  const acctText = acct
    ? [acct.email, acct.plan].filter(Boolean).join(" · ")
    : "";
  const acctLine = acctText
    ? `<div class="account">${escapeHTML(acctText)}</div>`
    : "";
  return `
    <footer class="footer">
      <div class="src-label">${escapeHTML(label)}</div>
      ${acctLine}
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
      void refresh();
      break;
    case "theme":
      cycleTheme();
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
  }
});

installStoneTexture();
applyTheme();
render();
void refresh();

// Tauri-only wiring: re-fetch when the popover opens, and on a slow timer.
// Skipped in showcase/browser mode (no Tauri runtime).
if (!MOCK) {
  listen("refresh", () => void refresh());
  setInterval(() => void refresh(), POLL_MS);
}
