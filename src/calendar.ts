import type { CalendarEntry, CalendarReport } from "./types";
import { escapeHTML as esc, formatUsageCost, formatTokens } from "./util";

export type CalendarScale = "days" | "months";
export type CalendarMetric = "tokens" | "cost";
export interface CalendarFilters { tool: string; model: string; account: string }
export interface CalendarBucket {
  key: string;
  tokens: number;
  cost: number;
  complete: boolean;
  estimated: boolean;
  entries: CalendarEntry[];
}
export function calendarBuckets(report: CalendarReport, scale: CalendarScale, filters: CalendarFilters): Map<string, CalendarBucket> {
  const buckets = new Map<string, CalendarBucket>();
  for (const e of report.entries) {
    if (!e.day.startsWith(`${report.year}-`) || (filters.tool !== "all" && e.tool !== filters.tool)
      || (filters.model !== "all" && e.model !== filters.model)
      || (filters.account !== "all" && e.account_key !== filters.account)) continue;
    const key = e.day.slice(0, scale === "months" ? 7 : 10);
    const b = buckets.get(key) ?? { key, tokens: 0, cost: 0, complete: true, estimated: false, entries: [] };
    b.tokens += e.tokens;
    b.cost += e.cost ?? 0;
    b.complete &&= e.cost !== null && e.cost_complete !== false;
    b.estimated ||= e.cost_estimated === true;
    b.entries.push(e);
    buckets.set(key, b);
  }
  return buckets;
}
const monthName = (year: number, month: number, long = false) => new Date(Date.UTC(year, month, 1))
  .toLocaleDateString("en", { month: long ? "long" : "short", timeZone: "UTC" });
const dateKey = (year: number, month: number, day = 1) => `${year}-${String(month + 1).padStart(2, "0")}-${String(day).padStart(2, "0")}`;
function costLabel(b: CalendarBucket): string {
  return formatUsageCost(b.cost, b.complete, b.estimated);
}
export function calendarDetailHTML(label: string, b?: CalendarBucket): string {
  if (!b?.entries.length) return `<strong>${esc(label)}</strong><div>No recorded usage</div>`;
  const models = new Map<string, { name: string; tokens: number; cost: number; complete: boolean; estimated: boolean }>();
  const members = new Map<string, { name: string; tokens: number; cost: number; complete: boolean; estimated: boolean }>();
  for (const e of b.entries) {
    const m = models.get(e.model) ?? { name: e.display_name, tokens: 0, cost: 0, complete: true, estimated: false };
    m.tokens += e.tokens; m.cost += e.cost ?? 0; m.complete &&= e.cost !== null && e.cost_complete !== false; m.estimated ||= e.cost_estimated === true;
    models.set(e.model, m);
    if (e.member_id) {
      const member = members.get(e.member_id) ?? { name: e.member || "Team member", tokens: 0, cost: 0, complete: true, estimated: false };
      member.tokens += e.tokens;
      member.cost += e.cost ?? 0;
      member.complete &&= e.cost !== null && e.cost_complete !== false;
      member.estimated ||= e.cost_estimated === true;
      members.set(e.member_id, member);
    }
  }
  const sorted = [...models.values()].sort((a, b) => b.tokens - a.tokens);
  const modelRows = sorted.slice(0, 5).map(m => `<div class="calendar-tip-row"><span>${esc(m.name)}</span><span>${formatTokens(m.tokens)} · ${formatUsageCost(m.cost, m.complete, m.estimated)}</span></div>`).join("");
  const memberRows = [...members.values()].sort((a, b) => b.tokens - a.tokens).slice(0, 3)
    .map(m => `<div class="calendar-tip-row"><span>${esc(m.name)}</span><span>${formatTokens(m.tokens)} · ${formatUsageCost(m.cost, m.complete, m.estimated)}</span></div>`).join("");
  return `<strong>${esc(label)}</strong><div class="calendar-tip-total">${formatTokens(b.tokens)} tokens · ${esc(costLabel(b))}</div>
    <div class="calendar-tip-section">${modelRows}${sorted.length > 5 ? `<div>+${sorted.length - 5} more models</div>` : ""}</div>
    ${memberRows ? `<div class="calendar-tip-section">${memberRows}${members.size > 3 ? `<div>+${members.size - 3} more members</div>` : ""}</div>` : ""}`;
}

export interface CalendarArgs {
  report: CalendarReport | null;
  year: number;
  month: number; // zero-based, UTC
  scale: CalendarScale;
  metric: CalendarMetric;
  filters: CalendarFilters;
  source: "local" | "team";
  error: string;
  loading: boolean;
  now?: Date;
}
export function calendarHTML(a: CalendarArgs): string {
  const now = a.now ?? new Date();
  const today = now.toISOString().slice(0, 10);
  const currentYear = now.getUTCFullYear(), currentMonth = now.getUTCMonth();
  const label = a.scale === "days" ? `${monthName(a.year, a.month, true)} ${a.year}` : `${a.year}`;
  const prevDisabled = a.year <= 2000 && (a.scale === "months" || a.month === 0);
  const nextDisabled = a.year >= currentYear && (a.scale === "months" || a.month >= currentMonth);
  const controls = `<div class="calendar-nav"><button class="calendar-arrow" data-action="calendar-prev" aria-label="Previous ${a.scale === "days" ? "month" : "year"}" ${prevDisabled ? "disabled" : ""}>‹</button>
    <span title="UTC">${label}</span><button class="calendar-arrow" data-action="calendar-next" aria-label="Next ${a.scale === "days" ? "month" : "year"}" ${nextDisabled ? "disabled" : ""}>›</button>
    <div class="calendar-scale">${(["days", "months"] as const).map(s => `<button data-action="calendar-scale" data-value="${s}" aria-pressed="${a.scale === s}">${s === "days" ? "Days" : "Months"}</button>`).join("")}</div></div>`;
  const report = a.report?.year === a.year ? a.report : null;
  if (!report) return `<section class="usage-calendar" aria-busy="${a.loading}">${controls}<div class="msg">${a.error ? `<button class="detail-link refresh-error" data-action="refresh" title="${esc(a.error)}">Retry</button>` : '<span aria-label="Loading calendar">…</span>'}</div></section>`;
  const relevant = report.entries.filter(e => a.filters.tool === "all" || e.tool === a.filters.tool);
  const modelNames = new Map(relevant.map(e => [e.model, e.display_name]));
  const accounts = new Map(report.entries.map(e => [e.account_key, e.account_label || "Unknown account"]));
  const select = (field: string, all: string, selected: string, options: Map<string, string>) =>
    `<select class="calendar-filter" data-calendar-filter="${field}" aria-label="Calendar ${field}"><option value="all">${all}</option>${[...options].sort((a,b) => a[1].localeCompare(b[1])).map(([id, name]) => `<option value="${esc(id)}" ${selected === id ? "selected" : ""}>${esc(name)}</option>`).join("")}</select>`;
  const metric = `<select class="calendar-filter calendar-metric" data-calendar-metric aria-label="Calendar heatmap metric">${(["tokens", "cost"] as const).map(m => `<option value="${m}" ${a.metric === m ? "selected" : ""}>${m === "tokens" ? "Tokens" : "Cost"}</option>`).join("")}</select>`;
  const filters = `<div class="calendar-filters">${select("model", "All models", a.filters.model, modelNames)}${a.source === "team" && accounts.size > 1 ? select("account", "All accounts", a.filters.account, accounts) : ""}${metric}</div>`;
  const buckets = calendarBuckets(report, a.scale, a.filters);
  const visible = [...buckets.values()].filter(b => a.scale === "months" || b.key.startsWith(dateKey(a.year, a.month).slice(0, 7)));
  // Never silently switch units as pricing coverage or the source changes.
  const useCost = a.metric === "cost";
  const magnitude = (b: CalendarBucket) => useCost ? b.cost : b.tokens;
  const peak = Math.max(Number.EPSILON, ...visible.map(magnitude));
  const cell = (key: string, text: string, fullLabel: string, future: boolean): string => {
    const b = buckets.get(key);
    const level = b && magnitude(b) > 0 ? Math.min(4, Math.max(1, Math.ceil(Math.sqrt(magnitude(b) / peak) * 4))) : 0;
    const description = b ? `${formatTokens(b.tokens)} tokens, ${costLabel(b)}` : "No recorded usage";
    return `<button class="calendar-cell level-${level}${key === today ? " calendar-today" : ""}" ${key === today ? 'aria-current="date"' : ""} ${future ? 'disabled aria-label="Future date"' : `data-calendar-tip="${esc(calendarDetailHTML(fullLabel, b))}" aria-label="${esc(`${fullLabel}: ${description}`)}"`}
      ${a.scale === "months" ? `data-action="calendar-month" data-value="${Number(key.slice(5, 7)) - 1}"` : ""} data-calendar-key="${key}"><span>${esc(text)}</span></button>`;
  };
  let grid: string;
  if (a.scale === "days") {
    const first = (new Date(Date.UTC(a.year, a.month, 1)).getUTCDay() + 6) % 7;
    const days = new Date(Date.UTC(a.year, a.month + 1, 0)).getUTCDate();
    grid = `<div class="calendar-grid calendar-days">${["M", "T", "W", "T", "F", "S", "S"].map(d => `<span class="calendar-weekday">${d}</span>`).join("")}${'<span aria-hidden="true"></span>'.repeat(first)}${Array.from({ length: days }, (_, i) => {
      const key = dateKey(a.year, a.month, i + 1);
      return cell(key, `${i + 1}`, `${monthName(a.year, a.month)} ${i + 1}, ${a.year}`, key > today);
    }).join("")}</div>`;
  } else {
    grid = `<div class="calendar-grid calendar-months">${Array.from({ length: 12 }, (_, m) => cell(dateKey(a.year, m).slice(0, 7), monthName(a.year, m), `${monthName(a.year, m, true)} ${a.year}`, a.year === currentYear && m > currentMonth)).join("")}</div>`;
  }
  const sum = visible.reduce((s, b) => ({ tokens: s.tokens + b.tokens, cost: s.cost + b.cost, complete: s.complete && b.complete, estimated: s.estimated || b.estimated }), { tokens: 0, cost: 0, complete: true, estimated: false });
  const summary = visible.length ? `${formatTokens(sum.tokens)} tokens · ${costLabel({ ...sum, key: "", entries: [] })}` : "No recorded usage";
  return `<section class="usage-calendar" aria-busy="${a.loading}">${controls}${filters}${a.error ? `<button class="detail-link refresh-error" data-action="refresh" title="${esc(a.error)} · Showing cached history">Refresh failed · Retry</button>` : ""}${grid}
    <div class="calendar-summary"><span title="API-rate estimates. ~ reconstructed from shared token counts; + incomplete pricing.">${esc(summary)}</span><span class="calendar-legend" role="img" aria-label="Low to high ${useCost ? "estimated cost" : "token usage"}" title="Low to high ${useCost ? "estimated cost" : "token usage"}">${[0,1,2,3,4].map(n => `<i class="level-${n}"></i>`).join("")}</span></div></section>`;
}

/** One overlay for the whole panel. Mouse, keyboard focus and touch/click all
 * reveal identical details; fixed positioning prevents clipping/resizing. */
export function installCalendarTooltip(root: HTMLElement): () => void {
  const tip = document.createElement("div");
  tip.className = "calendar-tooltip"; tip.id = "calendar-tooltip"; tip.role = "tooltip"; tip.hidden = true;
  document.body.append(tip);
  let active: HTMLElement | null = null;
  const hide = () => { active?.removeAttribute("aria-describedby"); active = null; tip.hidden = true; };
  const show = (target: EventTarget | null) => {
    const cell = target instanceof Element ? target.closest<HTMLElement>("[data-calendar-tip]") : null;
    if (!cell) return;
    hide(); active = cell; cell.setAttribute("aria-describedby", tip.id);
    tip.innerHTML = cell.dataset.calendarTip ?? ""; tip.hidden = false;
    const r = cell.getBoundingClientRect();
    const width = Math.min(document.documentElement.clientWidth, root.getBoundingClientRect().right);
    tip.style.maxWidth = `${Math.min(300, width - 16)}px`;
    tip.style.left = `${Math.max(8, Math.min(r.left, width - tip.offsetWidth - 8))}px`;
    const below = r.bottom + 8;
    tip.style.top = `${Math.max(8, below + tip.offsetHeight < innerHeight ? below : r.top - tip.offsetHeight - 8)}px`;
  };
  root.addEventListener("mouseover", e => show(e.target));
  root.addEventListener("focusin", e => show(e.target));
  root.addEventListener("click", e => { if ((e.target as Element).closest("[data-calendar-tip]")) show(e.target); else hide(); });
  root.addEventListener("mouseout", e => { if (!(e.relatedTarget instanceof Node) || !active?.contains(e.relatedTarget)) hide(); });
  root.addEventListener("focusout", hide);
  document.addEventListener("keydown", e => { if (e.key === "Escape") hide(); });
  document.addEventListener("scroll", hide, true);
  return hide; // Call before replacing the panel so detached cells never leave stale details.
}
