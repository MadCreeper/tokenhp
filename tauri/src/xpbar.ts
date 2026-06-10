// Minecraft Bedrock-style XP bar rendered as a pixel grid (same technique as
// hearts.ts): a capsule with stepped rounded ends and a thick black border,
// green fill with a pale highlight stripe, mottled dark texture in the empty
// portion, and dark segment dividers.

import { clamp01, escapeHTML } from "./util";

// Grid: W×H cells. Rows render at 1px/cell (12px tall, full panel width) —
// the finer vertical grid lets the border be 3px while the bar stays slim.
const H = 12;
const W = 163;
// Cells trimmed from each end per row — small corner steps only, so the bar
// reads as a rectangle with rounded corners (not a bullet/capsule).
const INSETS = [2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2];
// Border thickness in cells. Cells are ~2px wide × 1px tall, so the
// horizontal count is smaller to keep the drawn border visually even.
const BORDER_Y = 3;
const BORDER_X = 2;
const SEGMENTS = 15;

const BORDER_COLOR = "#0f0f0f";
// Interior fill rows top→bottom: highlight stripe up top, darker toward the
// bottom — the classic XP-bar shading.
const FILL_ROWS = [
  "#7fd321",
  "#c6f97a",
  "#c6f97a",
  "#7fd321",
  "#5fb010",
  "#447d09",
];
// Mottled dark texture for the drained portion (hash-picked per cell).
const EMPTY_SHADES = ["#1d2412", "#161b0e", "#232b16", "#11150a"];
const DIVIDER_ON_FILL = "#365c06";
const DIVIDER_ON_EMPTY = "#0b0e07";

// Interior x range is [2, W-3] on the straight rows.
const INTERIOR_X0 = 2;
const INTERIOR_W = W - 4;

const DIVIDERS = new Set<number>(
  Array.from({ length: SEGMENTS - 1 }, (_, k) =>
    INTERIOR_X0 + Math.round(((k + 1) * INTERIOR_W) / SEGMENTS),
  ),
);

function inShape(x: number, y: number): boolean {
  if (y < 0 || y >= H || x < 0 || x >= W) return false;
  const i = INSETS[y];
  return x >= i && x <= W - 1 - i;
}

/** Within the border distances of the outside → part of the black outline ring. */
function isBorder(x: number, y: number): boolean {
  for (let dy = -BORDER_Y; dy <= BORDER_Y; dy++) {
    for (let dx = -BORDER_X; dx <= BORDER_X; dx++) {
      if (!inShape(x + dx, y + dy)) return true;
    }
  }
  return false;
}

function cellColor(x: number, y: number, fillX: number): string {
  if (isBorder(x, y)) return BORDER_COLOR;
  const filled = x < fillX;
  if (DIVIDERS.has(x)) return filled ? DIVIDER_ON_FILL : DIVIDER_ON_EMPTY;
  if (filled) return FILL_ROWS[y - BORDER_Y] ?? FILL_ROWS[FILL_ROWS.length - 1];
  const key = (Math.imul(x, 49157) ^ Math.imul(y, 98317)) >>> 0;
  return EMPTY_SHADES[key & 3];
}

/** The bar itself as an SVG (no label/level text). */
export function xpBarSVG(value: number): string {
  const fillX = INTERIOR_X0 + Math.round(clamp01(value) * INTERIOR_W);
  let rects = "";
  for (let y = 0; y < H; y++) {
    const xe = W - 1 - INSETS[y];
    let x = INSETS[y];
    // Merge horizontal runs of identical color to keep the SVG small.
    while (x <= xe) {
      const color = cellColor(x, y, fillX);
      let x2 = x;
      while (x2 + 1 <= xe && cellColor(x2 + 1, y, fillX) === color) x2++;
      rects += `<rect x="${x}" y="${y}" width="${x2 - x + 1}" height="1" fill="${color}"/>`;
      x = x2 + 1;
    }
  }
  return `<svg class="xp-svg" viewBox="0 0 ${W} ${H}" preserveAspectRatio="none" shape-rendering="crispEdges" xmlns="http://www.w3.org/2000/svg">${rects}</svg>`;
}

/**
 * One XP bar row. `label` sits above-left; `trailing` (e.g. "12k · $0.34")
 * floats centered over the bar's top edge — MC's signature level placement.
 */
export function xpBar(label: string, value: number, trailing: string): string {
  return `
    <div class="xp">
      <div class="xp-title">${escapeHTML(label)}</div>
      <div class="xp-cluster">
        ${xpBarSVG(value)}
        <div class="xp-level">${escapeHTML(trailing)}</div>
      </div>
    </div>`;
}
