// Visual theme state. Mirrors the Swift app's @AppStorage("visualTheme"),
// extended to three themes. Persisted in localStorage; applied as a body
// class (`theme-<id>`) that styles.css keys all chrome off of.

export type Theme = "minecraft" | "classic" | "arknights";

const KEY = "hpbar-theme";
const ORDER: Theme[] = ["minecraft", "classic", "arknights"];

export function getTheme(): Theme {
  const t = localStorage.getItem(KEY);
  return ORDER.includes(t as Theme) ? (t as Theme) : "minecraft";
}

export function cycleTheme(): Theme {
  const next = ORDER[(ORDER.indexOf(getTheme()) + 1) % ORDER.length];
  localStorage.setItem(KEY, next);
  applyTheme(next);
  return next;
}

export function applyTheme(t: Theme = getTheme()): void {
  document.body.className = `theme-${t}`;
}

/** Short label for the theme-cycle button. */
export function themeLabel(t: Theme): string {
  return { minecraft: "MC", classic: "HP", arknights: "AK" }[t];
}
