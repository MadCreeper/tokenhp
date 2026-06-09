// Port of the SwiftUI `MinecraftXPBar`: a dark segmented track with a bright
// green fill (lighter top half, darker bottom) and the trailing value rendered
// centered above the bar in MC's green-with-black-outline style.

import { clamp01, escapeHTML } from "./util";

/**
 * One XP bar. `label` sits above-left; `trailing` (e.g. "12k · $0.34") floats
 * centered over the top edge of the bar.
 */
export function xpBar(label: string, value: number, trailing: string): string {
  const pct = clamp01(value) * 100;
  return `
    <div class="xp">
      <div class="xp-title">${escapeHTML(label)}</div>
      <div class="xp-cluster">
        <div class="xp-bar"><div class="xp-fill" style="width:${pct.toFixed(2)}%"></div></div>
        <div class="xp-level">${escapeHTML(trailing)}</div>
      </div>
    </div>`;
}
