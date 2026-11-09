// Theme-native header controls: pixel geometry, macOS linework, and AK solid gear / circular return arrows.
import type { Theme } from "./theme";

// ---------------------------------------------------------------- Minecraft

/** Build a crisp pixel SVG from a 16×16 predicate (true = filled pixel). */
function pixelSVG(fill: (x: number, y: number) => boolean): string {
  const N = 16;
  let rects = "";
  for (let y = 0; y < N; y++) {
    for (let x = 0; x < N; x++) {
      if (fill(x, y)) rects += `<rect x="${x}" y="${y}" width="1" height="1"/>`;
    }
  }
  return `<svg viewBox="0 0 ${N} ${N}" shape-rendering="crispEdges" fill="currentColor" aria-hidden="true" xmlns="http://www.w3.org/2000/svg">${rects}</svg>`;
}

// Hand-placed pixels keep the gear's square teeth and square hub readable at
// native size, rather than rasterising a generic round cog.
function mcGear(): string {
  const rows = [
    "......####......",
    "......####......",
    "..##..####..##..",
    "..############..",
    "...##########...",
    "...##########...",
    "######....######",
    "######....######",
    "######....######",
    "######....######",
    "...##########...",
    "...##########...",
    "..############..",
    "..##..####..##..",
    "......####......",
    "......####......",
  ];
  return pixelSVG((x, y) => rows[y][x] === "#");
}

// Bedrock-style square return arrow: right-angle track and a stepped head.
function mcRefresh(): string {
  return `<svg viewBox="0 0 16 16" fill="currentColor" shape-rendering="crispEdges" aria-hidden="true" xmlns="http://www.w3.org/2000/svg"><path d="M3 2h8v2H4v8h8V8h2v6H2V3h1z"/><path d="M10 0h2v2h2v2h2v2h-6z"/></svg>`;
}

// ---------------------------------------------------------------- Arknights
// Lucide v1.22.0 (ISC). `settings` (gear) and `refresh-cw` (two arrows).

const akLine = (paths: string): string =>
  `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" xmlns="http://www.w3.org/2000/svg">${paths}</svg>`;

const CLASSIC_GEAR = akLine(
  `<path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915"/><circle cx="12" cy="12" r="3"/>`,
);

// A compact macOS-style clockwise arrow, with rounded ends.
const CLASSIC_REFRESH = akLine(
  `<path d="M19.5 8A8 8 0 1 0 20 14"/><path d="M19.5 3.5V8H15"/>`,
);

// Arknights' home controls use solid mechanical shapes and fine circular
// tracks. These are authored SVGs, not the generic rounded Lucide controls.
const AK_GEAR = `<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" xmlns="http://www.w3.org/2000/svg"><path fill-rule="evenodd" d="M9 1h6l.6 3 1.5.9 2.9-1 3 5.2-2.3 2v1.8l2.3 2-3 5.2-2.9-1-1.5.9-.6 3H9l-.6-3-1.5-.9-2.9 1-3-5.2 2.3-2v-1.8L1 9.1l3-5.2 2.9 1L8.4 4z M16.5 12a4.5 4.5 0 1 0-9 0 4.5 4.5 0 1 0 9 0"/></svg>`;

// Home-screen assistant switch (beside the eye): a fine circular frame
// enclosing two heavy return arrows. The frame stays still while loading.
const AK_REFRESH = `<svg viewBox="0 0 28 28" fill="none" aria-hidden="true" xmlns="http://www.w3.org/2000/svg">
  <circle cx="14" cy="14" r="12" fill="#414141" stroke="#d7d7d7" stroke-width="1"/>
  <g class="ak-refresh-arrows" stroke="currentColor" stroke-width="2.3">
    <path d="M8.2 13a6 6 0 0 1 10-3.4M19.8 15a6 6 0 0 1-10 3.4"/>
    <path fill="currentColor" stroke="none" d="m5.1 12.1 6.4 0-3.2 3.3z M22.9 15.9h-6.4l3.2-3.3z"/>
  </g>
</svg>`;

// Lucide `pin`. Classic reuses it too: a monochrome line pin suits its macOS
// look better than the colour 📌 emoji would among the plain glyphs.
const AK_PIN = akLine(
  `<path d="M12 17v5"/><path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/>`,
);

/** A blocky pushpin: round head, collar, needle. */
function mcPin(): string {
  return pixelSVG((x, y) => {
    const dx = x + 0.5 - 8;
    const dy = y + 0.5 - 5;
    if (Math.hypot(dx, dy) <= 3.4) return true; // head
    if (y === 9 && x >= 5 && x <= 10) return true; // collar
    if (y >= 10 && y <= 12 && (x === 7 || x === 8)) return true; // needle
    return y === 13 && x === 8; // needle tip
  });
}

// ---------------------------------------------------------------- dispatch

export function settingsIcon(theme: Theme): string {
  if (theme === "minecraft") return mcGear();
  if (theme === "arknights") return AK_GEAR;
  return CLASSIC_GEAR;
}

export function refreshIcon(theme: Theme): string {
  if (theme === "minecraft") return mcRefresh();
  if (theme === "arknights") return AK_REFRESH;
  return CLASSIC_REFRESH;
}

export function pinIcon(theme: Theme): string {
  if (theme === "minecraft") return mcPin();
  return AK_PIN;
}
