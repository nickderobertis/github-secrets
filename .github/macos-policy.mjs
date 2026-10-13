// Exit 0: classification (unreadable PRs and unknown events require Mac); exit 1: invalid usage.
// One release-only authority for production routing and graph journeys.
import { execFileSync } from 'node:child_process';
import { readFileSync, realpathSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { parse } from 'yaml';
import { options } from './inputs.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const git = (args, cwd) => execFileSync('git', args, { cwd, encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'] });
const TOML_TO_JSON = 'console.log(JSON.stringify(Bun.TOML.parse(await Bun.stdin.text())))';
function toml(text) {
  if (globalThis.Bun) return globalThis.Bun.TOML.parse(text);
  // bash execs the pinned bun itself: on Windows scripts/bun.sh prints a POSIX
  // path that only the shell, not Node, can execute.
  return JSON.parse(execFileSync('bash', ['-c', 'exec "$(bash scripts/bun.sh path)" -e "$0"', TOML_TO_JSON], { cwd: root, input: text, encoding: 'utf8' }));
}
function version(v) {
  if (typeof v !== 'string') return false;
  const match = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/.exec(v);
  return !!match && (!match[4] || match[4].split('.').every(id => !/^\d+$/.test(id) || id === '0' || !id.startsWith('0')));
}
const equal = (a, b) => JSON.stringify(a) === JSON.stringify(b);
// release-please names its release-PR branch `release-please--branches--<branch
// it releases from>` (plus a component suffix); derive it from release.yml.
export function releaseBranchPrefix(configRoot = root) {
  const workflow = parse(readFileSync(configRoot + '.github/workflows/release.yml', 'utf8'));
  const config = JSON.parse(readFileSync(configRoot + 'release-please-config.json', 'utf8'));
  const branch = workflow?.on?.push?.branches;
  if (!Array.isArray(branch) || branch.length !== 1 || typeof branch[0] !== 'string' || !/^[A-Za-z0-9._/-]+$/.test(branch[0]) || config?.packages?.['.']?.['release-type'] !== 'rust') throw new Error('unsupported release configuration');
  return `release-please--branches--${branch[0]}`;
}
function semanticVersions(a, b, path, names) {
  if (path.endsWith('Cargo.toml')) {
    const old = toml(a), next = toml(b);
    if (!names.has(old.package?.name) || old.package.name !== next.package?.name) return false;
    if (!version(old.package.version) || !version(next.package.version)) return false;
    delete old.package.version; delete next.package.version;
    return equal(old, next);
  }
  if (path === 'Cargo.lock') {
    const old = toml(a), next = toml(b);
    for (const doc of [old, next]) for (const pkg of doc.package ?? []) {
      if (names.has(pkg.name) && !pkg.source) { if (!version(pkg.version)) return false; delete pkg.version; }
    }
    return equal(old, next);
  }
  if (path === '.release-please-manifest.json') {
    const old = JSON.parse(a), next = JSON.parse(b);
    return equal(Object.keys(old), Object.keys(next)) && Object.keys(old).every(k => k === '.' && version(old[k]) && version(next[k]));
  }
  return false;
}
const KNOWN_NON_PR = ['push', 'release', 'workflow_run', 'repository_dispatch', 'schedule', 'workflow_dispatch'];
export function classify(eventName, payload, base, head, cwd = process.cwd()) {
  if (KNOWN_NON_PR.includes(eventName)) return { isReleaseOnly: false, macosRequired: false, reason: `non-PR event ${eventName}` };
  if (eventName !== 'pull_request') return { isReleaseOnly: false, macosRequired: true, reason: `unrecognized event ${JSON.stringify(String(eventName))}: Mac coverage required` };
  const ordinary = reason => ({ isReleaseOnly: false, macosRequired: true, reason: `ordinary PR: ${reason}` });
  try {
    // Authority is the head branch and the diff alone, never the author or title.
    const pr = payload.pull_request, prefix = releaseBranchPrefix();
    if (!pr || !pr.base?.repo?.full_name || pr.head?.repo?.full_name !== pr.base.repo.full_name) return ordinary('not a same-repository head');
    if (typeof pr.head.ref !== 'string' || (pr.head.ref !== prefix && !pr.head.ref.startsWith(prefix + '--'))) return ordinary(`head branch is not under ${prefix}`);
    if (!base || !head || base.startsWith('-') || head.startsWith('-')) throw new Error('missing or invalid refs');
    // Compare from the fork point, as the PR's own diff does.
    base = git(['merge-base', base, head], cwd).trim();
    const paths = git(['diff', '--name-status', '-z', base, head, '--'], cwd).split('\0');
    const files = [];
    for (let i = 0; i < paths.length - 1; i += 2) {
      if (paths[i] !== 'M') return ordinary(`${paths[i + 1]} added, deleted or renamed`);
      files.push(paths[i + 1]);
    }
    if (!files.length) return ordinary('empty diff');
    const manifests = git(['ls-tree', '-r', '--name-only', base], cwd).trim().split('\n').filter(p => /(^|\/)Cargo.toml$/.test(p));
    const names = new Set(manifests.map(p => toml(git(['show', `${base}:${p}`], cwd)).package?.name).filter(Boolean));
    for (const path of files) {
      if (path === 'CHANGELOG.md') continue;
      if (!manifests.includes(path) && !['Cargo.lock', '.release-please-manifest.json'].includes(path)) return ordinary(`${path} is not version metadata`);
      if (!semanticVersions(git(['show', `${base}:${path}`], cwd), git(['show', `${head}:${path}`], cwd), path, names)) return ordinary(`${path} changes more than its own version`);
    }
    return { isReleaseOnly: true, macosRequired: false, reason: 'same-repository release-please branch; semantic version/changelog-only diff' };
  } catch (err) {
    return { isReleaseOnly: false, macosRequired: true, reason: `classification unavailable, Mac coverage required: ${err.message.split('\n')[0]}` };
  }
}
if (process.argv[1] && realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  if (args.shift() !== 'classify') throw new Error('usage: macos-policy.mjs classify --event PATH --base REF --head REF');
  const opts = options(args, ['--event', '--base', '--head'], ['--base', '--head']);
  let payload = {};
  try { payload = JSON.parse(readFileSync(opts['--event'], 'utf8')); } catch { /* A PR must fail closed. */ }
  // Without Actions' event identity, classify conservatively as an unknown PR.
  const event = process.env.GITHUB_EVENT_NAME || 'pull_request';
  console.log(JSON.stringify(classify(event, payload, opts['--base'], opts['--head'])));
}
