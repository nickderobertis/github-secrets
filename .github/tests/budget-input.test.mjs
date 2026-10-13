import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { root, digest } from '../workflow-graph.mjs';
import { history } from './helpers.mjs';
test('budget reporter rejects unknown argv and missing, stale or malformed telemetry', () => {
 const h = history();
 try {
  const path = join(h.cwd, '.git/telemetry.json');
  const run = args => spawnSync(process.execPath, [join(root, '.github/macos-budget.mjs'), ...args], { encoding: 'utf8' });
  for (const args of [[], ['--bogus', 'value'], ['--metric', 'jobs-per-run', '--metric', 'jobs-per-run'], ['--telemetry', path, '--metric']]) assert.notEqual(run(args).status, 0);
  const args = ['--metric', 'jobs-per-run', '--telemetry', path];
  assert.notEqual(run(args).status, 0);
  writeFileSync(path, JSON.stringify({ schemaVersion: 1, sourceDigest: 'stale' }));
  assert.match(run(args).stderr, /stale telemetry/);
  const job = { job: 'macos/macos', target: 'aarch64-apple-darwin', runner: 'macos-latest', enabled: true, macos: true };
  const mac = { workflow: 'ci.yml', event: 'opened/source/same-repo', jobs: [job] };
  const lifecycle = { ownership: 'same-repo', registryCredential: 'absent', runs: [mac] };
  for (const collections of [
   { scenarios: [], lifecycles: [] },
   { scenarios: [{ id: 'bad', runs: [{ workflow: 'ci', event: 'pr', jobs: [{ macos: 'false' }] }] }], lifecycles: [lifecycle] },
   // A lifecycle with no runs would report zero Mac-spending runs.
   { scenarios: [{ id: 'opened/source/same-repo', runs: [mac] }], lifecycles: [{ ownership: 'same-repo', registryCredential: 'absent', runs: [] }] },
   { scenarios: [{ id: 'opened/source/same-repo', runs: [mac] }], lifecycles: [{ runs: [mac] }] },
  ]) {
   writeFileSync(path, JSON.stringify({ schemaVersion: 1, sourceDigest: digest(), ...collections }));
   assert.match(run(args).stderr, /invalid telemetry collections/);
  }
  writeFileSync(path, JSON.stringify({ schemaVersion: 1, sourceDigest: digest(), scenarios: [{ id: 'scheduled', runs: [] }], lifecycles: [lifecycle] }));
  assert.match(run(args).stderr, /empty workflow graph/);
  writeFileSync(path, JSON.stringify({ schemaVersion: 1, sourceDigest: digest(), scenarios: [{ id: 'opened/source/same-repo', runs: [mac] }], lifecycles: [lifecycle] }));
  assert.match(run(['--metric', 'mac-minutes', '--telemetry', path]).stderr, /unknown metric/);
  const result = join(h.cwd, '.git/result.json');
  const ok = spawnSync(process.execPath, [join(root, '.github/macos-budget.mjs'), '--metric', 'runs-per-change', '--telemetry', path], { encoding: 'utf8', env: { ...process.env, ONEBUDGETSPEC_RESULT: result } });
  assert.equal(ok.status, 0, ok.stderr); assert.equal(ok.stdout, 'runs-per-change: 1\n');
  const reported = JSON.parse(readFileSync(result, 'utf8'));
  assert.equal(reported.value, 1);
  assert.deepEqual(JSON.parse(reported.detail), {
   scenarios: [{ event: 'opened/source/same-repo', runs: [{ workflow: 'ci.yml', jobs: [{ job: 'macos/macos', target: 'aarch64-apple-darwin', enabled: true }] }] }],
   lifecycles: [{ ownership: 'same-repo', registryCredential: 'absent', macosRuns: [{ workflow: 'ci.yml', event: 'opened/source/same-repo' }] }],
  });
 } finally { h.cleanup(); }
});
test('the telemetry digest follows real tracked, untracked and deleted source but not ignored telemetry', () => {
 const h = history();
 try {
  writeFileSync(join(h.cwd, '.gitignore'), '.telemetry/\n'); h.commit();
  const before = digest(h.cwd);
  assert.equal(digest(h.cwd), before);
  mkdirSync(join(h.cwd, '.telemetry')); writeFileSync(join(h.cwd, '.telemetry/macos-policy.json'), '{}');
  assert.equal(digest(h.cwd), before);
  assert.ok(h.git('ls-files', 'Cargo.toml')); writeFileSync(join(h.cwd, 'Cargo.toml'), '[package]\nname = "gh-secrets"\nversion = "9.9.9"\n');
  const edited = digest(h.cwd); assert.notEqual(edited, before);
  writeFileSync(join(h.cwd, 'new.rs'), 'fn added() {}');
  const added = digest(h.cwd); assert.notEqual(added, edited);
  rmSync(join(h.cwd, 'CHANGELOG.md'));
  assert.notEqual(digest(h.cwd), added);
 } finally { h.cleanup(); }
});
