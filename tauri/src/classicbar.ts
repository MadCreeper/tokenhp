// The "Classic" theme — port of the Swift DefaultTheme/NeutralTheme +
// HealthBarStyle: a rounded rectangle bar whose fill color ramps
// green → yellow → red as the REMAINING value drains (HP-bar semantics),
// and a single accent color for magnitude (mana-style) bars.

import { clamp01, escapeHTML } from "./util";

// RGB stops for the ramp (HealthBarStyle.swift).
const GREEN: [number, number, number] = [0.24, 0.8, 0.28];
const YELLOW: [number, number, number] = [0.95, 0.79, 0.3];
const RED: [number, number, number] = [0.91, 0.29, 0.24];

const ACCENT = "#0a84ff"; // NeutralTheme's accentColor

function mix(
  a: [number, number, number],
  b: [number, number, number],
  t: number,
): [number, number, number] {
  const k = clamp01(t);
  return [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k, a[2] + (b[2] - a[2]) * k];
}

function css(rgb: [number, number, number], alpha = 1): string {
  const [r, g, b] = rgb.map((v) => Math.round(v * 255));
  return alpha >= 1 ? `rgb(${r},${g},${b})` : `rgba(${r},${g},${b},${alpha})`;
}

/** Base color for a remaining value: upper half yellow→green, lower red→yellow. */
export function rampColor(value: number): [number, number, number] {
  const v = clamp01(value);
  return v >= 0.5 ? mix(YELLOW, GREEN, (v - 0.5) / 0.5) : mix(RED, YELLOW, v / 0.5);
}

function barHTML(
  title: string,
  fillFrac: number,
  fillColor: [number, number, number] | string,
  trailing: string,
  caption: string | null,
): string {
  const grad =
    typeof fillColor === "string"
      ? fillColor
      : `linear-gradient(to bottom, ${css(fillColor, 0.75)}, ${css(fillColor)})`;
  return `
    <div class="cbar">
      <div class="cbar-head">
        <span class="cbar-title">${escapeHTML(title)}</span>
        <span class="cbar-trailing">${escapeHTML(trailing)}</span>
      </div>
      <div class="cbar-track">
        <div class="cbar-fill" style="width:${(clamp01(fillFrac) * 100).toFixed(1)}%;background:${grad}"></div>
      </div>
      ${caption ? `<div class="cbar-caption">${escapeHTML(caption)}</div>` : ""}
    </div>`;
}

/** Quota (drain) bar — fill = remaining, color ramps with it. */
export function classicQuotaBar(
  title: string,
  remaining: number,
  trailing: string,
  caption: string | null,
): string {
  return barHTML(title, remaining, rampColor(remaining), trailing, caption);
}

/** Magnitude bar — single accent color; width already encodes the value. */
export function classicNeutralBar(title: string, frac: number, trailing: string): string {
  return barHTML(title, frac, ACCENT, trailing, null);
}
