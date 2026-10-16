import assert from 'node:assert/strict';
import { test } from 'node:test';
import { build } from 'esbuild';
const result = await build({ entryPoints: ['src/calendar.ts'], bundle: true, write: false, format: 'esm', platform: 'node' });
const { calendarBuckets, calendarHTML, calendarDetailHTML } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
const entry = (overrides = {}) => ({ day: '2024-02-01', model: 'a', display_name: 'Model A', tool: 'codex', member: 'Alice', member_id: '1', account_key: 'work', account_label: 'Work', tokens: 100, cost: 1, ...overrides });
const report = { year: 2024, entries: [entry(), entry({ day: '2024-02-29', cost: 2 }), entry({ day: '2024-03-01', model: 'b', tool: 'claude-code', account_key: 'private', tokens: 500, cost: null })] };
const filters = { tool: 'all', model: 'all', account: 'all' };
const args = { report, year: 2024, month: 1, scale: 'days', filters, source: 'team', error: '', loading: false, now: new Date('2024-03-15T00:00:00Z') };

test('daily and monthly aggregation agree without crossing month boundaries', () => {
  const days = calendarBuckets(report, 'days', filters);
  const months = calendarBuckets(report, 'months', filters);
  assert.equal(days.size, 3);
  assert.equal(months.get('2024-02').tokens, 200);
  assert.equal(months.get('2024-02').cost, 3);
  assert.equal(months.get('2024-03').tokens, 500);
  assert.equal([...days.values()].reduce((s, d) => s + d.tokens, 0), [...months.values()].reduce((s, d) => s + d.tokens, 0));
});

test('tool, account and model filters intersect in every bucket', () => {
  const b = calendarBuckets(report, 'months', { tool: 'codex', model: 'a', account: 'work' });
  assert.equal(b.size, 1); assert.equal(b.get('2024-02').cost, 3);
  assert.equal(calendarBuckets(report, 'months', { ...filters, model: 'b', account: 'work' }).size, 0);
});

test('leap day exists, future dates are disabled, and monthly mode has twelve cells', () => {
  const html = calendarHTML(args);
  assert.equal((html.match(/data-calendar-key=/g) ?? []).length, 29);
  assert.match(html, /2024-02-29/);
  assert.doesNotMatch(html, /2024-02-30/);
  const current = calendarHTML({ ...args, month: 2 });
  assert.match(current, /disabled aria-label="Future date"[^>]*\s+data-calendar-key="2024-03-16"/);
  assert.match(current, /data-action="calendar-next"[^>]*disabled/);
  const monthly = calendarHTML({ ...args, scale: 'months' });
  assert.equal((monthly.match(/data-calendar-key=/g) ?? []).length, 12);
});

test('unpriced data stays incomplete when mixed with priced records', () => {
  const r = { year: 2024, entries: [entry(), entry({ model: 'unpriced', cost: null })] };
  const b = calendarBuckets(r, 'days', filters).get('2024-02-01');
  assert.equal(b.cost, 1); assert.equal(b.complete, false);
  const detail = calendarDetailHTML('February 1', b);
  assert.match(detail, /partial pricing/); assert.match(detail, /unpriced/);
  assert.match(detail, /Alice/);
});

test('missing history is distinct from zero-dollar usage; stale-year data never renders', () => {
  assert.match(calendarDetailHTML('February 2'), /No recorded usage/);
  const stale = calendarHTML({ ...args, year: 2023 });
  assert.match(stale, /Loading calendar/); assert.doesNotMatch(stale, /data-calendar-key/);
  assert.match(calendarHTML({ ...args, report: null, error: 'Unavailable' }), /Unavailable/);
  assert.match(calendarHTML({ ...args, error: 'Offline' }), /Showing cached history/);
});

test('model, account and contributor labels cannot inject tooltip markup', () => {
  const evil = '<img src=x onerror=alert(1)>';
  const r = { year: 2024, entries: [entry({ display_name: evil, member: evil, account_label: evil })] };
  const b = calendarBuckets(r, 'days', filters).get('2024-02-01');
  assert.doesNotMatch(calendarDetailHTML(evil, b), /<img/);
  assert.match(calendarDetailHTML(evil, b), /&lt;img/);
  assert.doesNotMatch(calendarHTML({ ...args, report: r }), /<img/);
});
