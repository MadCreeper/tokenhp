import assert from 'node:assert/strict';
import { test } from 'node:test';
import { build } from 'esbuild';
const bundle = await build({ entryPoints: ['src/team-data.ts'], bundle: true, write: false, format: 'esm', platform: 'node' });
const { TeamReports } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
const deferred = () => { let resolve, reject; const promise = new Promise((a,b) => { resolve=a; reject=b; }); return { promise, resolve, reject }; };
const report = (range, tokens) => ({ range, members: [{tokens}] });

test('rapid day/week/day changes coalesce by range and never reuse another range', async () => {
  const calls = [];
  const day = deferred(), week = deferred();
  const store = new TeamReports(range => { calls.push(range); return range === 'day' ? day.promise : week.promise; });
  const a = store.fetch('day'), b = store.fetch('week'), c = store.fetch('day');
  assert.equal(a, c); assert.deepEqual(calls, ['day', 'week']);
  assert.equal(store.get('week'), null);
  week.resolve(report('week', 700)); await b;
  assert.equal(store.get('day'), null);
  day.resolve(report('day', 100)); await a;
  assert.equal(store.get('day').members[0].tokens, 100);
  assert.equal(store.get('week').members[0].tokens, 700);
});

test('a failed range can be retried without contaminating another range', async () => {
  let attempts = 0;
  const store = new TeamReports(async range => {
    if (range === 'month' && ++attempts === 1) throw new Error('connection lost');
    return report(range, range === 'month' ? 3000 : 100);
  });
  await store.fetch('day');
  await assert.rejects(store.fetch('month'), /connection lost/);
  assert.equal(store.get('month'), null);
  assert.equal(store.get('day').members[0].tokens, 100);
  await store.fetch('month'); assert.equal(store.get('month').members[0].tokens, 3000);
});

test('refresh replaces cached data only for its range', async () => {
  let count = 0;
  const store = new TeamReports(async range => report(range, ++count));
  await store.fetch('day'); await store.fetch('week'); await store.fetch('day');
  assert.equal(store.get('day').members[0].tokens, 3);
  assert.equal(store.get('week').members[0].tokens, 2);
});

test('changing team configuration invalidates late responses from the old team', async () => {
  const old = deferred(); let attempts = 0;
  const store = new TeamReports(range => ++attempts === 1 ? old.promise : Promise.resolve(report(range, 900)));
  const previous = store.fetch('day'); store.clear();
  await store.fetch('day'); old.resolve(report('day', 100)); await previous;
  assert.equal(store.get('day').members[0].tokens, 900);
});

test('mismatched server ranges are rejected instead of mislabelling totals', async () => {
  const store = new TeamReports(async () => report('day', 100));
  await assert.rejects(store.fetch('month'), /does not match/);
  assert.equal(store.get('month'), null);
});
