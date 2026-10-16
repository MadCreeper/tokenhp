// Run before packaging so beta/alpha tags remain part of the running version.
import { readFileSync, appendFileSync } from 'node:fs';
const tag = process.env.GITHUB_REF_NAME ?? '';
const version = /^v(\d+\.\d+\.\d+(?:-(?:alpha|beta)(?:\.\d+)?)?)$/.exec(tag)?.[1];
if (process.env.GITHUB_REF_TYPE !== 'tag' || !version) {
  throw new Error('Release builds require a vX.Y.Z, vX.Y.Z-beta[.N], or vX.Y.Z-alpha[.N] tag.');
}
const core = version.split('-')[0];
const cargoVersion = /^version = "([^"]+)"/m.exec(readFileSync('src-tauri/Cargo.toml', 'utf8'))?.[1];
const appVersion = JSON.parse(readFileSync('src-tauri/tauri.conf.json', 'utf8')).version;
const packageVersion = JSON.parse(readFileSync('package.json', 'utf8')).version;
if ([cargoVersion, appVersion, packageVersion].some(v => v !== core)) {
  throw new Error(`Release tag ${tag} does not match Cargo, Tauri, and package versions.`);
}
appendFileSync(process.env.GITHUB_ENV, `HPBAR_RELEASE_VERSION=${version}\n`);
console.log(`Release version: ${version}`);
