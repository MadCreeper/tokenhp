// Pixel-perfect port of the SwiftUI `HeartPixel` / `MinecraftHeartsBar`.
// A heart is a 7×7 grid drawn as a crisp SVG; a bar is 10 hearts that drain
// pixel-by-pixel from the right as `value` (the remaining fraction) decreases.

// 7×7 grid. Codes: 0 transparent, 1 outline, 2 body, 3 sparkle (highlight).
const PATTERN: number[][] = [
  [0, 1, 1, 0, 1, 1, 0],
  [1, 2, 2, 1, 2, 2, 1],
  [1, 3, 2, 2, 2, 2, 1],
  [1, 2, 2, 2, 2, 2, 1],
  [0, 1, 2, 2, 2, 1, 0],
  [0, 0, 1, 2, 1, 0, 0],
  [0, 0, 0, 1, 0, 0, 0],
];

// Colors. "Full" (remaining) is the bright Minecraft heart; "empty" (used by
// other devices) is darkened well below it so used-vs-left reads at a glance.
const OUTLINE = "#000000";
const BODY_FULL = "#e01c1c";
const SPARKLE_FULL = "#fff2f2";
const BODY_EMPTY = "#2e1a1a";
const SPARKLE_EMPTY = "#3d2424";

// Body pixels live in columns 1..5; map `fill` onto those 5 so a 1-column
// drain is actually visible (using all 7 made 0.86..1.0 look identical).
const FIRST_BODY_COL = 1;
const BODY_COL_COUNT = 5;

function clamp01(v: number): number {
  return Math.max(0, Math.min(1, v));
}

function pixelColor(code: number, filled: boolean): string | null {
  switch (code) {
    case 1:
      return OUTLINE;
    case 2:
      return filled ? BODY_FULL : BODY_EMPTY;
    case 3:
      return filled ? SPARKLE_FULL : SPARKLE_EMPTY;
    default:
      return null;
  }
}

/** SVG markup for a single heart filled by `fill` (0..1). */
export function heartSVG(fill: number): string {
  const f = clamp01(fill);
  const filledBodyCols = Math.max(
    0,
    Math.min(BODY_COL_COUNT, Math.round(f * BODY_COL_COUNT)),
  );
  const lastFilledCol = FIRST_BODY_COL + filledBodyCols - 1;

  let rects = "";
  for (let y = 0; y < PATTERN.length; y++) {
    const row = PATTERN[y];
    for (let x = 0; x < row.length; x++) {
      const color = pixelColor(row[x], x <= lastFilledCol);
      if (!color) continue;
      rects += `<rect x="${x}" y="${y}" width="1" height="1" fill="${color}"/>`;
    }
  }
  return `<svg class="heart" viewBox="0 0 7 7" shape-rendering="crispEdges" xmlns="http://www.w3.org/2000/svg">${rects}</svg>`;
}

/** Fraction of heart `index` that is full given the bar's overall `value`. */
export function fillForHeart(index: number, value: number): number {
  return clamp01(value * 10 - index);
}

/** A row of 10 draining hearts. */
export function heartsRow(value: number): string {
  let out = "";
  for (let i = 0; i < 10; i++) {
    out += heartSVG(fillForHeart(i, value));
  }
  return out;
}

// "Your drain" ghost tone — a dark red clearly below the bright full heart
// (so used reads apart from left), but above the near-black empty (so your drain
// reads apart from other devices').
const GHOST_BODY = "#6b2222";
const GHOST_SPARKLE = "#8a3838";

// --- Refill celebration ----------------------------------------------------
// A heart whose outline pixels carry a class (`ho`) so CSS can blink them
// black↔white during the refill animation.
function heartSVGCelebrate(fill: number): string {
  const f = clamp01(fill);
  const filledBodyCols = Math.max(0, Math.min(BODY_COL_COUNT, Math.round(f * BODY_COL_COUNT)));
  const lastFilledCol = FIRST_BODY_COL + filledBodyCols - 1;
  let rects = "";
  for (let y = 0; y < PATTERN.length; y++) {
    const row = PATTERN[y];
    for (let x = 0; x < row.length; x++) {
      const code = row[x];
      if (code === 1) {
        rects += `<rect class="ho" x="${x}" y="${y}" width="1" height="1" fill="${OUTLINE}"/>`;
        continue;
      }
      const color = pixelColor(code, x <= lastFilledCol);
      if (!color) continue;
      rects += `<rect x="${x}" y="${y}" width="1" height="1" fill="${color}"/>`;
    }
  }
  return `<svg class="heart" viewBox="0 0 7 7" shape-rendering="crispEdges" xmlns="http://www.w3.org/2000/svg">${rects}</svg>`;
}

/**
 * Two stacked heart rows for the refill animation: a back row of empty hearts
 * (the blinking-outline backdrop) and a front row filled to `remaining` that CSS
 * reveals left→right (the "refill"). Both rows' outlines blink via the `ho` class.
 */
export function heartsRefillHTML(remaining: number): string {
  let back = "";
  let front = "";
  for (let i = 0; i < 10; i++) {
    back += heartSVGCelebrate(0);
    front += heartSVGCelebrate(fillForHeart(i, remaining));
  }
  return `<div class="mc-celebrate"><div class="mc-back">${back}</div><div class="mc-front">${front}</div></div>`;
}

/** A full heart drawn in an explicit body/sparkle tone (outline unchanged). */
function tonedHeartSVG(body: string, sparkle: string): string {
  let rects = "";
  for (let y = 0; y < PATTERN.length; y++) {
    const row = PATTERN[y];
    for (let x = 0; x < row.length; x++) {
      const code = row[x];
      const color = code === 1 ? OUTLINE : code === 2 ? body : code === 3 ? sparkle : null;
      if (!color) continue;
      rects += `<rect x="${x}" y="${y}" width="1" height="1" fill="${color}"/>`;
    }
  }
  return `<svg class="heart" viewBox="0 0 7 7" shape-rendering="crispEdges" xmlns="http://www.w3.org/2000/svg">${rects}</svg>`;
}

/**
 * Like {@link heartsRow}, but the *drained* hearts are split into the portion
 * this machine used (a "ghost" mid-red) vs other devices (the dark empty tone) —
 * `machineShare` is this machine's fraction of the whole window. Bright hearts
 * still show remaining HP; the fractional edge heart is preserved.
 */
export function heartsRowSplit(remaining: number, machineShare: number): string {
  const fRem = clamp01(remaining);
  const fMac = clamp01(remaining + machineShare); // remaining + your-drain = 1 − others
  let out = "";
  for (let i = 0; i < 10; i++) {
    const lo = i / 10;
    const hi = (i + 1) / 10;
    const center = (i + 0.5) / 10;
    if (hi <= fRem + 1e-9) {
      out += heartSVG(1); // fully remaining → bright full
    } else if (lo >= fRem - 1e-9) {
      // fully drained → your-drain ghost, or others' drain (empty tone)
      out += center <= fMac ? tonedHeartSVG(GHOST_BODY, GHOST_SPARKLE) : heartSVG(0);
    } else {
      out += heartSVG(fillForHeart(i, remaining)); // straddles the HP edge
    }
  }
  return out;
}
