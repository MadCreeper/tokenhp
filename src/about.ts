// About + Update section rendering for the settings view. Pure string builders,
// same idiom as team.ts — they take explicit args so main.ts stays the single
// owner of state. The HTTP/download work lives Rust-side (the popover's CSP
// blocks frontend fetches); these just render and emit data-action buttons.

import type { AppControls, UpdateInfo } from "./types";
import { escapeHTML } from "./util";

export type SettingsSection = "general" | "update" | "team" | "about";
export type UpdateChannel = "stable" | "beta" | "alpha";

const REPO_URL = "https://github.com/MadCreeper/tokenhp";

const CHANNELS: { id: UpdateChannel; label: string }[] = [
  { id: "stable", label: "Stable" },
  { id: "beta", label: "Beta" },
  { id: "alpha", label: "Alpha" },
];

const CHANNEL_HELP: Record<UpdateChannel, string> = {
  stable: "Final releases only — the most tested builds.",
  beta: "Beta and stable releases. New features a bit earlier.",
  alpha: "Everything, including rough alpha builds. Expect bugs.",
};

/** The Update section: channel picker, a check button, and the result. */
export function updateSectionHTML(args: {
  channel: UpdateChannel;
  info: UpdateInfo | null;
  status: string;
  statusOk: boolean;
  checking: boolean;
  downloading: boolean;
  version: string;
}): string {
  const seg = CHANNELS.map(
    (c) =>
      `<button class="mc-btn ${c.id === args.channel ? "selected" : ""}" data-action="update-channel" data-value="${c.id}">${c.label}</button>`,
  ).join("");

  const status = args.checking
    ? `<div class="settings-status">Checking…</div>`
    : args.status
      ? `<div class="settings-status ${args.statusOk ? "ok" : "err"}">${escapeHTML(args.status)}</div>`
      : "";

  return `
    <div class="settings">
      <div class="settings-share-title">Release channel</div>
      <div class="seg settings-tabs">${seg}</div>
      <p class="settings-help">${CHANNEL_HELP[args.channel]}</p>

      <div class="settings-actions">
        <button class="mc-btn" data-action="update-check" ${args.checking ? "disabled" : ""}>Check for updates</button>
      </div>
      ${status}
      ${args.info ? updateResultHTML(args.info, args.downloading) : ""}

      <p class="about-fineprint">Currently running v${escapeHTML(args.version || args.info?.current || "?")}.</p>
    </div>`;
}

function updateResultHTML(info: UpdateInfo, downloading: boolean): string {
  const countLabel = `${info.count} release${info.count === 1 ? "" : "s"} on ${escapeHTML(info.channel)}`;

  // No newer build to install — but still show what the channel's latest *is*
  // (which differs per channel) so it's clear the check ran and against what.
  if (!info.available) {
    return `
      <div class="update-result">
        <div class="update-version">Latest on ${escapeHTML(info.channel)}: v${escapeHTML(info.latest)}</div>
        <p class="about-fineprint">Running v${escapeHTML(info.current)} — nothing newer to install · ${countLabel}.</p>
      </div>`;
  }

  // An update exists but ships no installer for this platform → send them to
  // the release page rather than a broken download.
  if (!info.has_asset) {
    return `
      <div class="update-result">
        <div class="update-version">v${escapeHTML(info.latest)} available</div>
        ${notesHTML(info.notes)}
        <div class="update-warn">No installer for this platform in that release.</div>
        <div class="settings-actions">
          <button class="mc-btn selected" data-action="open-url" data-value="${escapeHTML(info.html_url)}">Open release page</button>
        </div>
        <p class="about-fineprint">Running v${escapeHTML(info.current)} · ${countLabel}.</p>
      </div>`;
  }

  const size = info.asset_size ? ` · ${formatBytes(info.asset_size)}` : "";
  const btn = downloading
    ? `<button class="mc-btn selected" disabled>Downloading…</button>`
    : `<button class="mc-btn selected" data-action="update-install">Download &amp; install v${escapeHTML(info.latest)}</button>`;
  return `
    <div class="update-result">
      <div class="update-version">v${escapeHTML(info.latest)} available <span class="update-asset">${escapeHTML(info.asset_name)}${size}</span></div>
      ${notesHTML(info.notes)}
      <div class="settings-actions">${btn}</div>
      <p class="about-fineprint">Running v${escapeHTML(info.current)} · ${countLabel}. Downloads to your Downloads folder, then opens the installer. Builds are unsigned — see the README for the first-open caveat.</p>
    </div>`;
}

// Show the changelog, lightly trimmed. We don't render Markdown — just the raw
// text, scrollable if it's long.
function notesHTML(notes: string): string {
  const trimmed = notes.trim();
  if (!trimmed) return "";
  return `<div class="update-notes">${escapeHTML(trimmed)}</div>`;
}

/** The About section: identity, version, and links (opened OS-side). */
export function aboutSectionHTML(args: { version: string }): string {
  const link = (label: string, url: string) =>
    `<button class="mc-btn about-link" data-action="open-url" data-value="${escapeHTML(url)}">${escapeHTML(label)}</button>`;

  return `
    <div class="settings about">
      <div class="about-head">
        <div class="about-name">HPBar</div>
        <div class="about-version">v${escapeHTML(args.version || "?")}</div>
      </div>
      <p class="about-tagline">Your Claude (and Codex) subscription usage, live in the menu bar.</p>

      <div class="about-links">
        ${link("GitHub", REPO_URL)}
        ${link("Releases", `${REPO_URL}/releases`)}
        ${link("Report an issue", `${REPO_URL}/issues`)}
      </div>

      <p class="about-fineprint">Made by MadCreeper · MIT-licensed · unsigned builds.</p>
    </div>`;
}

/** The General section: app controls relocated from the (now macOS-absent) tray
 *  menu — Open at login, Quota alerts, single-device calibration — plus Quit.
 *  While `controls` is null (still loading) the toggles render disabled. Each
 *  toggle carries `data-field="ctl-<key>"`; main.ts's input handler routes those
 *  to the backend. Quit fires `data-action="quit-app"`. */
export function generalSectionHTML(args: {
  controls: AppControls | null;
  glass?: boolean;
}): string {
  const c = args.controls;
  // Backend-controlled toggles carry data-field="ctl-<key>" (routed to the
  // backend); `field`-form toggles (like glass) carry the raw field name and are
  // handled frontend-side. `disabled` gates the ctl ones until controls load.
  const toggle = (label: string, hint: string, field: string, on: boolean, disabled = false) => `
    <label class="settings-toggle ctl-toggle">
      <span class="settings-toggle-label">${escapeHTML(label)}<span class="ctl-hint">${escapeHTML(hint)}</span></span>
      <input type="checkbox" data-field="${field}" ${on ? "checked" : ""}${disabled ? " disabled" : ""} />
      <span class="settings-toggle-track"><span class="settings-toggle-knob"></span></span>
    </label>`;
  // Liquid Glass is macOS-only; only shown when a glass state is passed in.
  const glass =
    args.glass === undefined
      ? ""
      : toggle(
          "Liquid Glass",
          "Translucent widget-style backdrop (macOS)",
          "glass",
          args.glass,
        );
  return `
    <div class="settings general">
      ${glass}
      ${toggle("Open at login", "Launch HPBar when you sign in", "ctl-autostart", !!c?.autostart, !c)}
      ${toggle("Quota alerts", "Notify on low / critical quota", "ctl-alerts", !!c?.alerts, !c)}
      ${toggle("Only device here", "Assume this is your only active machine", "ctl-calibrate", !!c?.calibrate, !c)}
      <div class="general-foot">
        <button class="mc-btn general-quit" data-action="quit-app">Quit HPBar</button>
      </div>
    </div>`;
}

function formatBytes(n: number): string {
  if (n >= 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  if (n >= 1024) return `${Math.round(n / 1024)} KB`;
  return `${n} B`;
}
