// The "Arknights" theme — dark angular UI inspired by the game's resource
// readouts. The 5-hour session maps to 理智 (Sanity, the brain icon, shown as
// cur/135 like the in-game top bar); the weekly window maps to 源石
// (Originite Prime, the small gold gem); extra usage maps to 合成玉 (Orundum).
// Bars are thin, dark-tracked, and skewed — the game's signature diagonal cut.
//
// Icons are simplified hand-drawn approximations, not game assets.

import { clamp01, escapeHTML } from "./util";

// Bump when an /ak/*.png asset is replaced, to bust the webview's image cache.
const ICON_V = "3";

// 理智 — the actual in-game sanity icon (low-poly brain + lightning bolt),
// bundled as a PNG asset in public/ak/ (sourced from the Arknights wiki).
const ICON_SANITY = `<img src="/ak/sanity.png?v=${ICON_V}" alt="理智" draggable="false"/>`;

// 源石 (Originite Prime) and 合成玉 (Orundum) — the actual in-game item icons,
// bundled as PNG assets in public/ak/ (sourced from the Arknights wiki).
const ICON_ORIGINITE = `<img src="/ak/originite.png?v=${ICON_V}" alt="源石" draggable="false"/>`;
const ICON_ORUNDUM = `<img src="/ak/orundum.png?v=${ICON_V}" alt="合成玉" draggable="false"/>`;

interface Resource {
  icon: string;
  zh: string;
  max: number;
}

const RESOURCES: Record<string, Resource> = {
  "5-Hour": { icon: ICON_SANITY, zh: "理智", max: 135 },
  Weekly: { icon: ICON_ORIGINITE, zh: "源石", max: 100 },
  "Extra usage": { icon: ICON_ORUNDUM, zh: "合成玉", max: 100 },
};

/**
 * The 理智 hero block — replica of the Terminal screen's sanity readout:
 * a light-gray skewed plate with the "+" circle, the poly-brain icon, the big
 * current-sanity number, and the black 理智/135 tag, with a slim drain bar
 * along the plate's bottom edge.
 */
/** Ghost segment for the device split: this machine's share of the *drained*
 *  part, positioned from `remaining` to `remaining + machineShare`. */
function akGhost(remaining: number, machineShare: number | null | undefined): string {
  if (machineShare == null) return "";
  return `<div class="ak-fill-ghost" style="left:${(clamp01(remaining) * 100).toFixed(1)}%;width:${(clamp01(machineShare) * 100).toFixed(1)}%"></div>`;
}

function akSanityHero(
  remaining: number,
  caption: string | null,
  machineShare?: number | null,
): string {
  const v = clamp01(remaining);
  const cur = Math.round(v * 135);
  return `
    <div class="ak-hero">
      <div class="ak-hero-plate">
        <span class="ak-plus">+</span>
        <span class="ak-hero-brain">${ICON_SANITY}</span>
        <div class="ak-hero-mid">
          <div class="ak-hero-num">${cur}</div>
          <div class="ak-hero-tag">理智/135</div>
        </div>
        <div class="ak-hero-bar"><div class="ak-hero-fill" style="width:${(v * 100).toFixed(1)}%"></div>${akGhost(v, machineShare)}</div>
      </div>
      ${caption ? `<div class="ak-caption">${escapeHTML(caption)}</div>` : ""}
    </div>`;
}

/**
 * One live-quota resource row: icon + 理智-style name, big cur/max count on
 * the right, thin skewed bar underneath. `trailingOverride` (e.g. "Off")
 * replaces the count when the window has no meaningful fraction.
 * The 5-Hour window renders as the sanity hero plate instead.
 */
export function akResource(
  title: string,
  remaining: number,
  caption: string | null,
  trailingOverride: string | null,
  machineShare?: number | null,
): string {
  if (title === "5-Hour") return akSanityHero(remaining, caption, machineShare);
  const r = RESOURCES[title] ?? { icon: ICON_ORUNDUM, zh: title, max: 100 };
  const v = clamp01(remaining);
  const count = trailingOverride
    ? `<span class="ak-count ak-off">${escapeHTML(trailingOverride)}</span>`
    : `<span class="ak-count">${Math.round(v * r.max)}<span class="ak-max">/${r.max}</span></span>`;
  return `
    <div class="ak-row">
      <div class="ak-head">
        <span class="ak-icon">${r.icon}</span>
        <span class="ak-name">${escapeHTML(r.zh)} <span class="ak-sub">${escapeHTML(title)}</span></span>
        ${count}
      </div>
      <div class="ak-bar"><div class="ak-fill" style="width:${(v * 100).toFixed(1)}%"></div>${akGhost(v, machineShare)}</div>
      ${caption ? `<div class="ak-caption">${escapeHTML(caption)}</div>` : ""}
    </div>`;
}

/** Local-activity magnitude bar: label / trailing head + thin skewed bar. */
export function akBar(label: string, frac: number, trailing: string): string {
  return `
    <div class="akb">
      <div class="akb-head">
        <span class="akb-title">${escapeHTML(label)}</span>
        <span class="akb-trailing">${escapeHTML(trailing)}</span>
      </div>
      <div class="ak-bar"><div class="ak-fill" style="width:${(clamp01(frac) * 100).toFixed(1)}%"></div></div>
    </div>`;
}
