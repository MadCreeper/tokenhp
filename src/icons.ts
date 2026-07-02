// Header action icons (refresh, settings), drawn to match each visual theme so
// they stop reading as stray Unicode glyphs on the game-inspired themes:
//   • Minecraft → authored pixel art on a 16px grid (shape-rendering=crispEdges),
//     the same idiom as the hearts / XP bar.
//   • Arknights → Lucide line icons (ISC licensed), whose thin geometric stroke
//     matches the game's clean UI; see THIRD_PARTY_NOTICES.md.
//   • Classic   → the plain glyphs, which already suit its macOS-ish look.

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
  return `<svg viewBox="0 0 ${N} ${N}" shape-rendering="crispEdges" fill="currentColor" xmlns="http://www.w3.org/2000/svg">${rects}</svg>`;
}

/** A blocky 8-tooth cog with a hollow center. */
function mcGear(): string {
  const c = 8;
  const bodyR = 5.3; // solid disc
  const toothR = 7.2; // teeth reach
  const holeR = 2.5; // center hole
  return pixelSVG((x, y) => {
    const dx = x + 0.5 - c;
    const dy = y + 0.5 - c;
    const r = Math.hypot(dx, dy);
    if (r < holeR) return false;
    const a = Math.atan2(dy, dx);
    const inTooth = r <= toothR && Math.cos(8 * a) > 0.55;
    return r <= bodyR || inTooth;
  });
}

/** Two-arrow reload: a 2px ring broken by two gaps, each capped with an
 *  arrowhead. The second arrowhead is the first rotated 180° (point symmetry). */
function mcRefresh(): string {
  const O = 8; // grid center (corner coords)
  const rIn = 3.4;
  const rOut = 5.6;
  const rMid = 4.5;
  // Two arrowheads at the diagonal, each a triangle whose tip runs along the
  // clockwise tangent at a small gap in the ring, so the ring's two long arcs
  // read as arrows chasing each other.
  const headAngles = [-55, 125];
  type Pt = [number, number];
  const tris: [Pt, Pt, Pt][] = headAngles.map((d) => {
    const th = (d * Math.PI) / 180;
    const u: Pt = [Math.cos(th), Math.sin(th)]; // radial out
    const t: Pt = [-Math.sin(th), Math.cos(th)]; // clockwise tangent
    const ex = O + rMid * u[0];
    const ey = O + rMid * u[1];
    const tip: Pt = [ex + t[0] * 3.2, ey + t[1] * 3.2];
    const b1: Pt = [O + (rOut + 1.1) * u[0], O + (rOut + 1.1) * u[1]];
    const b2: Pt = [O + (rIn - 1.1) * u[0], O + (rIn - 1.1) * u[1]];
    return [tip, b1, b2];
  });
  const inTri = (px: number, py: number, a: Pt, b: Pt, c: Pt): boolean => {
    const cross = (ax: number, ay: number, bx: number, by: number, cx: number, cy: number) =>
      (ax - cx) * (by - cy) - (bx - cx) * (ay - cy);
    const d1 = cross(px, py, a[0], a[1], b[0], b[1]);
    const d2 = cross(px, py, b[0], b[1], c[0], c[1]);
    const d3 = cross(px, py, c[0], c[1], a[0], a[1]);
    const neg = d1 < 0 || d2 < 0 || d3 < 0;
    const pos = d1 > 0 || d2 > 0 || d3 > 0;
    return !(neg && pos);
  };
  const angDist = (deg: number, g: number) => Math.abs(((deg - g + 540) % 360) - 180);
  return pixelSVG((x, y) => {
    const px = x + 0.5;
    const py = y + 0.5;
    for (const [a, b, c] of tris) if (inTri(px, py, a, b, c)) return true;
    const dx = px - O;
    const dy = py - O;
    const r = Math.hypot(dx, dy);
    if (r < rIn || r > rOut) return false;
    const deg = (Math.atan2(dy, dx) * 180) / Math.PI;
    for (const g of headAngles) if (angDist(deg, g) < 24) return false; // gaps
    return true;
  });
}

// ---------------------------------------------------------------- Arknights
// Lucide v1.22.0 (ISC). `settings` (gear) and `refresh-cw` (two arrows).

const akLine = (paths: string): string =>
  `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" xmlns="http://www.w3.org/2000/svg">${paths}</svg>`;

const AK_GEAR = akLine(
  `<path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915"/><circle cx="12" cy="12" r="3"/>`,
);

const AK_REFRESH = akLine(
  `<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/>`,
);

// ---------------------------------------------------------------- dispatch

export function settingsIcon(theme: Theme): string {
  if (theme === "minecraft") return mcGear();
  if (theme === "arknights") return AK_GEAR;
  return "⚙";
}

export function refreshIcon(theme: Theme): string {
  if (theme === "minecraft") return mcRefresh();
  if (theme === "arknights") return AK_REFRESH;
  return "⟳";
}
