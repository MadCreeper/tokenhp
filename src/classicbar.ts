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

/** Same hue, lower saturation (mix toward its own grayscale), optionally dimmed —
 *  for the "used up" track/ghost so it harmonizes with the colorful fill instead
 *  of clashing as flat gray. */
function desat(
  [r, g, b]: [number, number, number],
  amount: number,
  dim = 1,
): [number, number, number] {
  const lum = 0.299 * r + 0.587 * g + 0.114 * b;
  const m = (c: number) => (c + (lum - c) * amount) * dim;
  return [m(r), m(g), m(b)];
}

function barHTML(
  title: string,
  fillFrac: number,
  fillColor: [number, number, number] | string,
  trailing: string,
  caption: string | null,
  trackColor?: string,
  ghost?: { left: number; width: number; color?: string },
  hurtFrom?: number | null,
): string {
  const grad =
    typeof fillColor === "string"
      ? fillColor
      : `linear-gradient(to bottom, ${css(fillColor, 0.75)}, ${css(fillColor)})`;
  const trackStyle = trackColor ? ` style="background:${trackColor}"` : "";
  // Ghost segment = this machine's share of the *drained* part (others = track).
  const ghostDiv = ghost
    ? `<div class="cbar-ghost" style="left:${(clamp01(ghost.left) * 100).toFixed(1)}%;width:${(clamp01(ghost.width) * 100).toFixed(1)}%${ghost.color ? `;background:${ghost.color}` : ""}"></div>`
    : "";
  // On a drop, animate the fill shrinking from its previous width (--hp-from).
  const hurt = hurtFrom != null ? " hp-drop" : "";
  const hurtVar = hurtFrom != null ? `;--hp-from:${(clamp01(hurtFrom) * 100).toFixed(1)}%` : "";
  return `
    <div class="cbar">
      <div class="cbar-head">
        <span class="cbar-title">${escapeHTML(title)}</span>
        <span class="cbar-trailing">${escapeHTML(trailing)}</span>
      </div>
      <div class="cbar-track"${trackStyle}>
        <div class="cbar-fill${hurt}" style="width:${(clamp01(fillFrac) * 100).toFixed(1)}%;background:${grad}${hurtVar}"></div>
        ${ghostDiv}
      </div>
      ${caption ? `<div class="cbar-caption">${escapeHTML(caption)}</div>` : ""}
    </div>`;
}

const WHITE: [number, number, number] = [1, 1, 1];

/** Quota (drain) bar — fill = remaining, color ramps with it. The "used up"
 *  track is the same hue at lower saturation (not gray); when `machineShare` is
 *  given, this machine's drain shows as a *lighter* (ghost-like) tint of the hue.
 *  `off` (disabled window, e.g. Extra usage) renders a neutral bar, not a
 *  colored/"depleted" one. */
export function classicQuotaBar(
  title: string,
  remaining: number,
  trailing: string,
  caption: string | null,
  machineShare?: number | null,
  off?: boolean,
  hurtFrom?: number | null,
): string {
  if (off) {
    return barHTML(title, 0, "transparent", trailing, caption, "rgba(0,0,0,0.14)");
  }
  const base = rampColor(remaining);
  const track = css(desat(base, 0.6, 0.9)); // used by others: same hue, low saturation
  const ghost =
    machineShare != null
      ? { left: remaining, width: machineShare, color: css(mix(base, WHITE, 0.5)) } // mine: lighter, ghostly
      : undefined;
  return barHTML(title, remaining, base, trailing, caption, track, ghost, hurtFrom);
}

/** Magnitude bar — single accent color; width already encodes the value. */
export function classicNeutralBar(title: string, frac: number, trailing: string): string {
  return barHTML(title, frac, ACCENT, trailing, null);
}

/** Refill-celebration bar: the fill grows from empty to `remaining` with a
 *  red→yellow→ramp hue shift (CSS `@keyframes classic-refill`). `--cl-normal` is
 *  the steady ramp color it settles to. */
export function classicRefillBar(title: string, remaining: number): string {
  const r = clamp01(remaining);
  const normal = css(rampColor(r));
  return `
    <div class="cbar">
      <div class="cbar-head">
        <span class="cbar-title">${escapeHTML(title)}</span>
        <span class="cbar-trailing">refilled</span>
      </div>
      <div class="cbar-track">
        <div class="cbar-fill celebrate-classic" style="--fill:${(r * 100).toFixed(1)}%;--cl-normal:${normal}"></div>
      </div>
    </div>`;
}
