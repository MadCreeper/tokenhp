// Visual theme state. Mirrors the Swift app's @AppStorage("visualTheme"),
// extended to three themes. Persisted in localStorage; applied as a body
// class (`theme-<id>`) that styles.css keys all chrome off of.

export type Theme = "minecraft" | "classic" | "arknights";

const KEY = "hpbar-theme";
const ORDER: Theme[] = ["minecraft", "classic", "arknights"];

// Forced theme (used by the showcase tool via ?theme=…); overrides storage.
let override: Theme | null = null;
export function setThemeOverride(t: Theme): void {
  override = t;
  applyTheme(t);
}

export function isTheme(s: string | null): s is Theme {
  return ORDER.includes(s as Theme);
}

export function getTheme(): Theme {
  if (override) return override;
  const t = localStorage.getItem(KEY);
  return isTheme(t) ? t : "minecraft";
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
