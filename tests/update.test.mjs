import assert from 'node:assert/strict';
import { test } from 'node:test';
import { build } from 'esbuild';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
const result = await build({ entryPoints: ['src/about.ts'], bundle: true, write: false, format: 'esm', platform: 'node' });
const { updateSectionHTML } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
const info = { current: '1.0.0-beta.1', latest: '1.0.0-beta.2', latest_tag: 'v1.0.0-beta.2', available: true,
  channel: 'beta', count: 2, notes: '', html_url: 'https://github.com/MadCreeper/tokenhp/releases/tag/v1.0.0-beta.2',
  published_at: '', asset_name: 'HPBar_1.0.0_universal.dmg', asset_url: 'https://github.com/MadCreeper/tokenhp/releases/download/v1.0.0-beta.2/HPBar_1.0.0_universal.dmg',
  asset_size: 7000000, asset_digest: null, has_asset: true };
const args = { channel: 'beta', info, status: '', statusOk: false, checking: false, downloading: false, version: '1.0.0-beta.1' };

test('only newer versions with a compatible installer offer download', () => {
  assert.match(updateSectionHTML(args), /data-action="update-install"/);
  assert.doesNotMatch(updateSectionHTML({ ...args, info: { ...info, available: false } }), /data-action="update-install"/);
  const missing = updateSectionHTML({ ...args, info: { ...info, has_asset: false } });
  assert.doesNotMatch(missing, /data-action="update-install"/);
  assert.match(missing, /data-action="open-url"/);
});

test('an active download disables another install, channel changes and checks', () => {
  const html = updateSectionHTML({ ...args, downloading: true });
  assert.doesNotMatch(html, /data-action="update-install"/);
  assert.equal((html.match(/data-action="update-channel"[^>]*disabled/g) ?? []).length, 3);
  assert.match(html, /data-action="update-check" disabled/);
});

test('release metadata and errors cannot inject markup', () => {
  const html = updateSectionHTML({ ...args, status: '<img onerror=alert(1)>', info: { ...info, notes: '<script>oops</script>', asset_name: '<img src=x>' } });
  assert.doesNotMatch(html, /<img|<script/);
  assert.match(html, /&lt;script&gt;/);
});

test('release packaging retains channel suffix and rejects mismatched tags', () => {
  const dir = mkdtempSync(join(tmpdir(), 'hpbar-release-version-'));
  const core = JSON.parse(readFileSync('package.json', 'utf8')).version;
  try {
    const run = (tag, type = 'tag') => spawnSync(process.execPath, ['scripts/release-version.mjs'], {
      encoding: 'utf8', env: { ...process.env, GITHUB_REF_NAME: tag, GITHUB_REF_TYPE: type, GITHUB_ENV: join(dir, 'env') },
    });
    for (const suffix of ['', '-beta', '-beta.10', '-alpha.2']) {
      const result = run(`v${core}${suffix}`);
      assert.equal(result.status, 0, result.stderr);
      assert.match(readFileSync(join(dir, 'env'), 'utf8'), new RegExp(`HPBAR_RELEASE_VERSION=${core}${suffix}\\n`));
    }
    for (const tag of ['v999.0.0', `v${core}-betamax`, 'main']) assert.notEqual(run(tag).status, 0);
    assert.notEqual(run(`v${core}`, 'branch').status, 0);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
