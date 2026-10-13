import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
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
  for (const collections of [{ scenarios: [], lifecycles: [] }, { scenarios: [{ id: 'bad', runs: [{ workflow: 'ci', event: 'pr', jobs: [{ macos: 'false' }] }] }], lifecycles: [{ runs: [] }] }]) {
   writeFileSync(path, JSON.stringify({ schemaVersion: 1, sourceDigest: digest(), ...collections }));
   assert.match(run(args).stderr, /invalid telemetry collections/);
  }
 } finally { h.cleanup(); }
});
