import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { UsageReport, UsageWindow } from "./types";
import { heartsRow } from "./hearts";
import "./styles.css";

const barsEl = document.getElementById("bars")!;
const sourceEl = document.getElementById("source")!;
const statusEl = document.getElementById("status")!;

// Refresh the live quota every 30 minutes, mirroring the Swift app's poll.
const POLL_MS = 30 * 60 * 1000;

let inFlight = false;

async function refresh(): Promise<void> {
  if (inFlight) return;
  inFlight = true;
  setStatus(barsEl.childElementCount === 0 ? "Loading…" : "");
  try {
    const report = await invoke<UsageReport>("fetch_usage");
    render(report);
    setStatus("");
  } catch (err) {
    // Keep the last good bars on screen; surface the error in the footer.
    setStatus(String(err), true);
  } finally {
    inFlight = false;
  }
}

function render(report: UsageReport): void {
  sourceEl.textContent = report.source_label;
  barsEl.innerHTML = report.windows.map(barHTML).join("");
}

function barHTML(w: UsageWindow): string {
  const trailing = w.trailing ?? `${Math.round(w.remaining * 100)}%`;
  const caption = resetCaption(w.resets_at);
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

/** "resets in 3h 12m" style caption from an RFC3339 timestamp. */
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

function setStatus(msg: string, isError = false): void {
  statusEl.textContent = msg;
  statusEl.classList.toggle("error", isError && msg !== "");
}

function escapeHTML(s: string): string {
  return s.replace(
    /[&<>"']/g,
    (c) =>
      ({
        "&": "&amp;",
        "<": "&lt;",
        ">": "&gt;",
        '"': "&quot;",
        "'": "&#39;",
      })[c]!,
  );
}

// Re-fetch whenever the popover is shown (Rust emits "refresh" on tray click)…
listen("refresh", () => void refresh());
// …on a slow background timer…
setInterval(() => void refresh(), POLL_MS);
// …and once on first load.
void refresh();
