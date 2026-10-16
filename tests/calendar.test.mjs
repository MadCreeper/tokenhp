import assert from 'node:assert/strict';
import { test } from 'node:test';
import { build } from 'esbuild';
const result = await build({ entryPoints: ['src/calendar.ts'], bundle: true, write: false, format: 'esm', platform: 'node' });
const { calendarBuckets, calendarHTML, calendarDetailHTML } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
const entry = (overrides = {}) => ({ day: '2024-02-01', model: 'a', display_name: 'Model A', tool: 'codex', member: 'Alice', member_id: '1', account_key: 'work', account_label: 'Work', tokens: 100, cost: 1, ...overrides });
const report = { year: 2024, entries: [entry(), entry({ day: '2024-02-29', cost: 2 }), entry({ day: '2024-03-01', model: 'b', tool: 'claude-code', account_key: 'private', tokens: 500, cost: null })] };
const filters = { tool: 'all', model: 'all', account: 'all' };
const args = { report, year: 2024, month: 1, scale: 'days', metric: 'tokens', filters, source: 'team', error: '', loading: false, now: new Date('2024-03-15T00:00:00Z') };

test('both sources show tokens and cost and use the chosen heatmap unit in both scales', () => {
  for (const source of ['local', 'team']) for (const scale of ['days', 'months']) {
    const firstDay = scale === 'days' ? '2024-02-01' : '2024-01-01';
    const secondDay = scale === 'days' ? '2024-02-02' : '2024-02-01';
    const r = { year: 2024, entries: [
      entry({ day: firstDay, tokens: 10000, cost: 1 }),
      entry({ day: secondDay, tokens: 100, cost: 1000, cost_complete: false }),
    ] };
    for (const metric of ['tokens', 'cost']) {
      const html = calendarHTML({ ...args, report: r, source, scale, metric });
      assert.match(html, /calendar-summary[\s\S]*10k tokens · \$1\.0k\+/);
      assert.match(html, new RegExp(`option value="${metric}" selected`));
      assert.match(html, new RegExp(`Low to high ${metric === 'cost' ? 'estimated cost' : 'token usage'}`));
      const cells = [...html.matchAll(/<button class="calendar-cell ([^"]+)"[^>]*data-calendar-key="([^"]+)"/g)];
      const key = firstDay.slice(0, scale === 'months' ? 7 : 10);
      assert.match(cells.find(c => c[2] === key)[1], metric === 'tokens' ? /level-4/ : /level-1/);
      const bucket = calendarBuckets(r, scale, filters).get(key);
      assert.match(calendarDetailHTML('Selected', bucket), /10k tokens · \$1\.00/);
    }
  }
});

test('member details include costs and preserve partial, estimated and unpriced status', () => {
  const r = { year: 2024, entries: [
    entry({ tokens: 1000, cost: 2, cost_estimated: true }),
    entry({ tokens: 500, cost: null }),
    entry({ member_id: '2', member: 'Bob', tokens: 200, cost: null }),
  ] };
  const html = calendarDetailHTML('Selected', calendarBuckets(r, 'days', filters).get('2024-02-01'));
  assert.match(html, /<span>Alice<\/span><span>2k · ~\$2\.00\+<\/span>/);
  assert.match(html, /<span>Bob<\/span><span>200 · unpriced<\/span>/);
});

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
  assert.match(detail, /\$1\.00\+/); assert.match(detail, /unpriced/);
  assert.match(detail, /Alice/);
});

test('partial and recovered prices survive daily/monthly totals and model filters', () => {
  const r = { year: 2024, entries: [
    entry({ cost: 3.2, cost_complete: false, cost_estimated: true }),
    entry({ cost: 2, day: '2024-02-02', cost_complete: true }),
    entry({ model: 'codex-auto-review', cost: null }),
  ] };
  for (const scale of ['days', 'months']) {
    const buckets = calendarBuckets(r, scale, filters);
    assert.equal([...buckets.values()].reduce((s, b) => s + b.cost, 0), 5.2);
    const first = [...buckets.values()][0];
    assert.equal(first.complete, false);
    assert.equal(first.estimated, true);
    assert.match(calendarDetailHTML('History', first), /~\$[35]\.20\+/);
    assert.match(calendarDetailHTML('History', first), /unpriced/);
  }
  const selected = calendarBuckets(r, 'months', { ...filters, model: 'a' }).get('2024-02');
  assert.equal(selected.cost, 5.2);
  assert.equal(selected.complete, false);
  assert.doesNotMatch(calendarDetailHTML('History', selected), /unpriced/);
  const html = calendarHTML({ ...args, report: r });
  assert.match(html, /~\$5\.20\+/);
  assert.doesNotMatch(html, /partial pricing/);
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
