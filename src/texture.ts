// Port of MinecraftBevel.drawStone (MinecraftThemes.swift): subtle horizontal
// stone-grain streaks — short 3-6px runs, 2px tall, scattered sparsely and
// keyed off pixel coordinates so the pattern is stable. Generated once as a
// tiling SVG data-URI and applied to every stone button via a CSS variable.

const TILE_W = 56; // multiple of segStep so the tile repeats seamlessly
const TILE_H = 24; // multiple of rowStep
const ROW_STEP = 3;
const SEG_STEP = 7;
const STREAK_H = 2;

function stoneTileSVG(): string {
  let rects = "";
  for (let y = 1; y < TILE_H - 1; y += ROW_STEP) {
    for (let x = 0; x < TILE_W; x += SEG_STEP) {
      // Same hash as the Swift version: (x*49157) ^ (y*98317).
      const key = (Math.imul(x, 49157) ^ Math.imul(y, 98317)) >>> 0;
      if ((key & 7) > 2) continue; // ~3/8 of slots get a streak
      const len = 3 + ((key >>> 3) & 3);
      const dark = (key & 1) === 0;
      rects += `<rect x="${x}" y="${y}" width="${Math.min(len, TILE_W - x)}" height="${STREAK_H}" fill="${dark ? "#000" : "#fff"}" fill-opacity="${dark ? 0.07 : 0.05}"/>`;
    }
  }
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${TILE_W}" height="${TILE_H}" shape-rendering="crispEdges">${rects}</svg>`;
}

/** Expose the tile to CSS as --stone-tile (used by .mc-btn). */
export function installStoneTexture(): void {
  const uri = `url("data:image/svg+xml;utf8,${encodeURIComponent(stoneTileSVG())}")`;
  document.documentElement.style.setProperty("--stone-tile", uri);
}
