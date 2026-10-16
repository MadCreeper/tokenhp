import assert from 'node:assert/strict';
import { test } from 'node:test';
import { build } from 'esbuild';

const result = await build({ entryPoints: ['src/team.ts'], bundle: true, write: false, format: 'esm', platform: 'node' });
const { teamContentHTML } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
const model = (id, tokens, cost, complete = true) => ({ model: id, display_name: id, tokens, cost, cost_complete: complete });
const account = (key, models) => ({ provider: 'claude', account_key: key, billing_key: key, account_label: key,
  attribution_status: 'exact', by_model: models, tokens: models.reduce((s, m) => s + m.tokens, 0), cost: models.reduce((s, m) => s + m.cost, 0) });
const a = model('model-A', 100, 10), b = model('model-B', 900, 90);
const member = { member_id: 'one', display_name: 'One', identity_version: 2, is_legacy: false, is_self: false,
  is_stale: false, current_project: null, last_seen_secs: 0, tokens: 1000, cost: 100, by_model: [a, b],
  by_project: [
    { project: 'B-only', tokens: 900, cost: 90, by_model: [b] },
    { project: 'A-only', tokens: 100, cost: 10, by_model: [a] },
  ], by_account: [account('shared', [a]), account('private', [b])],
};
const report = { team_name: 'Team', members: [member], models: [a, b].map(m => ({ id: m.model, display_name: m.model, tokens: m.tokens })), accounts: [] };
function render(overrides = {}) {
  return teamContentHTML({ report, model: 'all', dropdownOpen: false, breakdownMode: false,
    accountKey: 'all', selfName: '', expanded: new Set(['one']), topProjects: 1, error: '', theme: 'classic', ...overrides });
}

test('projects are filtered and ranked before applying top-N', () => {
  const html = render({ model: 'model-A' });
  assert.match(html, /A-only/);
  assert.doesNotMatch(html, /B-only|\$90|\$100/);
  assert.match(html, /\$10/);
  assert.match(render(), /B-only/);
});

test('model and account filters intersect in usage breakdown', () => {
  const html = render({ breakdownMode: true, model: 'model-A', accountKey: 'shared' });
  assert.match(html, /Usage breakdown/);
  assert.match(html, /100%/);
  assert.match(html, /estimated cost share/);
  assert.doesNotMatch(html, /model-B|private|\$90|\$100/);
  assert.match(html, /team-toggle-dropdown/); // model selection stays available
  const empty = render({ breakdownMode: true, model: 'model-B', accountKey: 'shared' });
  assert.match(empty, /no usage for this account/);
  assert.doesNotMatch(empty, /100%|\$90/);
});

test('a partially priced member cannot silently drive cost shares', () => {
  const incomplete = { ...member, by_model: [model('model-A', 100, 10, false), b] };
  const html = render({ breakdownMode: true, report: { ...report, members: [incomplete] } });
  assert.match(html, /token share.*complete pricing unavailable/);
});

test('shares recompute for the selected account and model', () => {
  const second = { ...member, member_id: 'two', display_name: 'Two', by_model: [model('model-A', 300, 30)],
    by_account: [account('shared', [model('model-A', 300, 30)])] };
  const html = render({ breakdownMode: true, model: 'model-A', accountKey: 'shared', report: { ...report, members: [member, second] } });
  assert.match(html, /25%/);
  assert.match(html, /75%/);
  assert.ok(html.indexOf('1. Two') < html.indexOf('2. One'));
});

test('missing legacy project-model data never shows unfiltered totals', () => {
  const legacy = { ...member, by_project: [{ project: 'legacy', tokens: 1000, cost: 100 }] };
  const html = render({ model: 'model-A', report: { ...report, members: [legacy] } });
  assert.doesNotMatch(html, /\$100/);
  assert.match(html, /no project activity/);
});
