// One release-only authority for production routing and graph journeys.
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { parse } from 'yaml';

const root = fileURLToPath(new URL('../', import.meta.url));
const git = (args, cwd) => execFileSync('git', args, { cwd, encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'] });
function toml(text) {
  const bun = execFileSync('bash', [root + 'scripts/bun.sh', 'path'], { encoding: 'utf8' }).trim();
  return JSON.parse(execFileSync(bun, ['-e', 'console.log(JSON.stringify(Bun.TOML.parse(await Bun.stdin.text())))'], { input: text, encoding: 'utf8' }));
}
const equal = (a, b) => JSON.stringify(a) === JSON.stringify(b);
export function releaseBranch() {
  const workflow = parse(readFileSync(root + '.github/workflows/release.yml', 'utf8'));
  const config = JSON.parse(readFileSync(root + 'release-please-config.json', 'utf8'));
  const branch = workflow.on.push.branches;
  if (branch.length !== 1 || config.packages['.']['release-type'] !== 'rust') throw new Error('unsupported release configuration');
  const name = toml(readFileSync(root + 'Cargo.toml', 'utf8')).package.name;
  return `release-please--branches--${branch[0]}--components--${name}`;
}
function semanticVersions(a, b, path, names) {
  if (path.endsWith('Cargo.toml')) {
    const old = toml(a), next = toml(b);
    if (!names.has(old.package?.name) || old.package.name !== next.package?.name) return false;
    delete old.package.version; delete next.package.version;
    return equal(old, next);
  }
  if (path === 'Cargo.lock') {
    const old = toml(a), next = toml(b);
    for (const doc of [old, next]) for (const pkg of doc.package ?? []) {
      if (names.has(pkg.name) && !pkg.source) delete pkg.version;
    }
    return equal(old, next);
  }
  if (path === '.release-please-manifest.json') {
    const old = JSON.parse(a), next = JSON.parse(b);
    return equal(Object.keys(old), Object.keys(next)) && Object.keys(old).every(k => k === '.' && typeof old[k] === 'string' && typeof next[k] === 'string' && /^\d+\.\d+\.\d+$/.test(next[k]));
  }
  return false;
}
export function classify(eventName, payload, base, head, cwd = process.cwd()) {
  const ordinary = { isReleaseOnly: false, macosRequired: eventName === 'pull_request', reason: 'ordinary PR or unknown classification' };
  if (eventName !== 'pull_request') return { ...ordinary, reason: 'non-PR event' };
  try {
    const pr = payload.pull_request;
    if (!pr || !pr.base?.repo?.full_name || pr.head?.repo?.full_name !== pr.base.repo.full_name || pr.head.ref !== releaseBranch()) return ordinary;
    if (!base || !head || base.startsWith('-') || head.startsWith('-')) throw new Error('missing or invalid refs');
    const paths = git(['diff', '--name-status', '-z', base, head, '--'], cwd).split('\0');
    const files = [];
    for (let i = 0; i < paths.length - 1; i += 2) {
      if (paths[i] !== 'M') return ordinary;
      files.push(paths[i + 1]);
    }
    if (!files.length) return ordinary;
    const manifests = git(['ls-tree', '-r', '--name-only', base], cwd).trim().split('\n').filter(p => /(^|\/)Cargo.toml$/.test(p));
    const names = new Set(manifests.map(p => toml(git(['show', `${base}:${p}`], cwd)).package?.name).filter(Boolean));
    for (const path of files) {
      if (path === 'CHANGELOG.md') continue;
      if (!manifests.includes(path) && !['Cargo.lock', '.release-please-manifest.json'].includes(path)) return ordinary;
      if (!semanticVersions(git(['show', `${base}:${path}`], cwd), git(['show', `${head}:${path}`], cwd), path, names)) return ordinary;
    }
    return { isReleaseOnly: true, macosRequired: false, reason: 'same-repository release-please branch; semantic version/changelog-only diff' };
  } catch (err) {
    return { ...ordinary, reason: `classification unavailable: ${err.message.split('\n')[0]}` };
  }
}
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  if (args.shift() !== 'classify') throw new Error('usage: macos-policy.mjs classify --event PATH --base REF --head REF');
  const options = {};
  while (args.length) { const key = args.shift(); options[key] = args.shift(); }
  let payload = {};
  try { payload = JSON.parse(readFileSync(options['--event'], 'utf8')); } catch { /* A PR must fail closed. */ }
  // Without Actions' event identity, classify conservatively as an unknown PR.
  const event = process.env.GITHUB_EVENT_NAME || 'pull_request';
  console.log(JSON.stringify(classify(event, payload, options['--base'], options['--head'])));
}
