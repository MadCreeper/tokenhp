export function clamp01(v: number): number {
  return Math.max(0, Math.min(1, v));
}

export function escapeHTML(s: string): string {
  return s.replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ]!,
  );
}

/** Port of the Swift `formatTokens`: 1234 → "1k", 1_200_000 → "1.2M". */
export function formatTokens(n: number): string {
  if (n < 1_000) return `${n}`;
  if (n < 1_000_000) return `${Math.round(n / 1_000)}k`;
  if (n < 1_000_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  return `${(n / 1_000_000_000).toFixed(2)}B`;
}

/** Port of the Swift `formatDollars`. */
export function formatDollars(d: number): string {
  if (d === 0) return "$0";
  if (d < 0.01) return "<$0.01";
  if (d < 1_000) return `$${d.toFixed(2)}`;
  if (d < 1_000_000) return `$${(d / 1_000).toFixed(1)}k`;
  return `$${(d / 1_000_000).toFixed(2)}M`;
}

/** Local hh:mm for the "Updated …" footer. */
export function nowTime(): string {
  return new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}
