import { execFileSync } from 'node:child_process';
import { mkdtempSync, writeFileSync, rmSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { root } from '../workflow-graph.mjs';
const scratch = process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? process.env.RUNNER_TEMP ?? process.cwd() + '/.github/.telemetry';
mkdirSync(scratch, { recursive: true });
export function history() {
  const cwd = mkdtempSync(join(scratch, 'macos-history-'));
  const git = (...args) => execFileSync('git', args, { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  git('init', '-q', '-b', 'master'); git('config', 'user.name', 'fixture'); git('config', 'user.email', 'fixture@example.invalid'); git('config', 'commit.gpgsign', 'false');
  const manifest = '[package]\nname = "gh-secrets"\nversion = "1.0.0"\nedition = "2021"\n[dependencies]\nserde = "1"\n';
  writeFileSync(join(cwd, 'Cargo.toml'), manifest);
  writeFileSync(join(cwd, 'Cargo.lock'), 'version = 4\n[[package]]\nname = "gh-secrets"\nversion = "1.0.0"\n[[package]]\nname = "serde"\nversion = "1.0.0"\nsource = "registry"\n');
  writeFileSync(join(cwd, '.release-please-manifest.json'), '{".":"1.0.0"}');
  writeFileSync(join(cwd, 'CHANGELOG.md'), '# Changelog\n');
  const commit = () => { git('add', '-A'); git('commit', '-qm', 'fixture'); return git('rev-parse', 'HEAD'); };
  const base = commit(); const heads = {};
  for (const kind of ['source', 'version', 'dependency']) {
    git('checkout', '-q', '-B', kind, base);
    if (kind === 'version') { writeFileSync(join(cwd, 'Cargo.toml'), manifest.replace('1.0.0', '1.1.0')); writeFileSync(join(cwd, '.release-please-manifest.json'), '{".":"1.1.0"}'); writeFileSync(join(cwd, 'CHANGELOG.md'), '# Changelog\nNew release\n'); }
    else if (kind === 'dependency') writeFileSync(join(cwd, 'Cargo.toml'), manifest.replace('serde = "1"', 'serde = "2"'));
    else writeFileSync(join(cwd, 'source.rs'), 'fn main() {}');
    heads[kind] = commit();
  }
  return { cwd, base, heads, git, commit, cleanup: () => rmSync(cwd, { recursive: true, force: true }) };
}
export function payload(h, kind, fork = false, release = true) {
  return { action: 'synchronize', pull_request: { title: 'chore(master): release 1.1.0', user: { login: 'nickderobertis' }, head: { ref: release ? 'release-please--branches--master--components--gh-secrets' : 'feature', sha: h.heads[kind], repo: { full_name: fork ? 'someone/fork' : 'nickderobertis/github-secrets' } }, base: { ref: 'master', sha: h.base, repo: { full_name: 'nickderobertis/github-secrets' } } } };
}
export function classify(h, p, event = 'pull_request', head = p.pull_request?.head?.sha) {
  const file = join(h.cwd, '.git', 'event.json'); writeFileSync(file, typeof p === 'string' ? p : JSON.stringify(p));
  return JSON.parse(execFileSync(process.execPath, [join(root, '.github/macos-policy.mjs'), 'classify', '--event', file, '--base', h.base, '--head', head ?? 'HEAD'], { cwd: h.cwd, encoding: 'utf8', env: { ...process.env, GITHUB_EVENT_NAME: event } }));
}
