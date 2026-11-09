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
  return isTheme(t) ? t : "classic";
}

export function setTheme(theme: Theme): void {
  override = null;
  localStorage.setItem(KEY, theme);
  applyTheme(theme);
}

export function applyTheme(t: Theme = getTheme()): void {
  document.body.className = `theme-${t}`;
}
