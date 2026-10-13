import test from 'node:test';
import { spawnSync } from 'node:child_process';
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { root, workflows, resolveRuns, digest, matrixRows, evaluate, interpolate } from '../workflow-graph.mjs';
import { history, payload, classify } from './helpers.mjs';
const required = ['check (macos-latest)', 'check (ubuntu-latest)', 'check (windows-latest)', 'build (macos-latest)', 'build (ubuntu-latest)', 'build (windows-latest)', 'live-e2e', 'live-e2e-bitwarden', 'llmlint', 'lint PR title'];
const classes = [
 ['opened', 'pull_request', 'opened'], ['synchronize', 'pull_request', 'synchronize'], ['reopened', 'pull_request', 'reopened'],
 ['base-push', 'push'], ['release-merge', 'push'], ['release-tag', 'push'], ['release-published', 'release', 'published'], ['downstream', 'workflow_run', 'completed'], ['repository-dispatch', 'repository_dispatch'], ['scheduled', 'schedule'], ['manual-default', 'workflow_dispatch'], ['manual-opt-in', 'workflow_dispatch'],
];
test('actual event/call/matrix graph and complete lifecycle record telemetry', () => {
 const h = history();
 try {
  const graph = workflows(); const scenarios = [];
  for (const kind of ['source', 'version', 'dependency']) for (const fork of [false, true]) for (const [id, event, action] of classes) {
   const p = payload(h, kind, fork); p.action = action;
   const decision = classify(h, p, event);
   const scenario = { id: `${id}/${kind}/${fork ? 'fork' : 'same-repo'}`, event, action, payload: p, ref: id === 'release-tag' ? 'refs/tags/v1.1.0' : 'refs/heads/master', macosRequired: decision.macosRequired, diagnostic: id === 'manual-opt-in', releaseCreated: id === 'release-merge', tag: 'v1.1.0' };
   const runs = resolveRuns(scenario, graph);
   if (event === 'pull_request') {
    const names = runs.flatMap(r => r.jobs.map(j => j.name));
    for (const name of required) assert.equal(names.filter(n => n === name).length, 1, `${scenario.id}: ${name}`);
    const mac = runs.flatMap(r => r.jobs).filter(j => j.macos);
    assert.equal(mac.some(j => j.job === 'macos'), decision.macosRequired);
   }
   if (event === 'push' && !scenario.releaseCreated) assert.equal(runs.flatMap(r => r.jobs).some(j => j.macos), false);
   if (scenario.releaseCreated) assert.equal(runs.flatMap(r => r.jobs).filter(j => j.macos).map(j => j.target).join(), 'aarch64-apple-darwin');
   if (['release', 'workflow_run', 'repository_dispatch', 'schedule'].includes(event)) assert.equal(runs.flatMap(r => r.jobs).some(j => j.macos), false);
   if (id === 'manual-default') assert.equal(runs.flatMap(r => r.jobs).some(j => j.macos), false);
   scenarios.push({ ...scenario, classification: decision, runs });
  }
  // Ordinary feature PR, its merge, generated release PR, its merge/build,
  // then every automatic downstream event. Diagnostics are expressly excluded.
  const lifecycles = [false, true].flatMap(fork => [false, true].map(publish => {
   const suffix = fork ? 'fork' : 'same-repo';
   const ids = [`synchronize/source/${suffix}`, `base-push/source/${suffix}`, 'synchronize/version/same-repo', 'release-merge/version/same-repo', 'release-tag/version/same-repo', 'release-published/version/same-repo', 'downstream/version/same-repo', 'repository-dispatch/version/same-repo', 'scheduled/version/same-repo'];
   return { ownership: suffix, optionalPublish: publish, runs: ids.flatMap(id => scenarios.find(s => s.id === id).runs) };
  }));
  mkdirSync(root + '.github/.telemetry', { recursive: true });
  writeFileSync(root + '.github/.telemetry/macos-policy.json', JSON.stringify({ schemaVersion: 1, sourceDigest: digest(), scenarios, lifecycles }, null, 2));
 } finally { h.cleanup(); }
});
test('unsupported runners, matrices and calls fail explicitly; actual matrix expansion handles include/exclude', () => {
 assert.deepEqual(matrixRows({ os: ['ubuntu-latest', 'macos-latest'], exclude: [{ os: 'ubuntu-latest' }], include: [{ os: 'macos-latest', target: 'darwin' }] }), [{ os: 'macos-latest', target: 'darwin' }]);
 const scenario = { id: 'fixture', event: 'pull_request', payload: {}, ref: 'refs/heads/feature' };
 for (const job of [{ 'runs-on': '${{ inputs.unknown }}' }, { uses: 'owner/repo/.github/workflows/test.yml@main' }, { 'runs-on': 'macos-latest', strategy: { matrix: '${{ inputs.matrix }}' } }]) assert.throws(() => resolveRuns(scenario, { 'test.yml': { on: { pull_request: null }, jobs: { test: job } } }), /unresolved|unsupported/);
 const graph = { 'caller.yml': { on: { pull_request: null }, jobs: { call: { uses: './.github/workflows/callee.yml' } } }, 'callee.yml': { on: { workflow_call: null }, jobs: { native: { 'runs-on': 'macos-latest' } } } };
 assert.equal(resolveRuns(scenario, graph)[0].jobs[0].macos, true);
});
test('preserved native proof and release artifact dependency contract', () => {
 const graph = workflows(); const ci = graph['ci.yml'].jobs, release = graph['release.yml'].jobs;
 assert.deepEqual(release.build.strategy.matrix.include.map(r => r.target).sort(), ['aarch64-apple-darwin', 'aarch64-unknown-linux-gnu', 'x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu'].sort());
 for (const name of ['Check', 'Build release binary', 'Build', 'Package (unix)', 'Install through scripts/install.sh', 'Run the installed binary']) assert.ok(ci.macos.steps.some(s => s.name === name), name);
 const install = release.build.steps.findIndex(s => s.name === 'Install staged native archive');
 const upload = release.build.steps.findIndex(s => s.uses?.startsWith('actions/upload-artifact@'));
 assert.ok(install >= 0 && install < upload);
 assert.equal(release.upload['runs-on'], 'ubuntu-latest');
 assert.deepEqual(release.upload.needs, ['release-please', 'build']);
 assert.deepEqual(release['crates-io'].needs, ['release-please', 'build']);
 assert.equal(release.build.steps[0].with.ref, '${{ needs.release-please.outputs.tag_name }}');
 for (const name of ['Package (unix)', 'Package (windows)']) {
  assert.equal(ci.install.steps.find(s => s.name === name).run, release.build.steps.find(s => s.name === name).run);
 }
 for (const id of ['check-macos', 'build-macos']) {
  assert.equal(ci[id].if, '${{ always() }}');
  assert.deepEqual(ci[id].needs, ['policy', 'macos', 'check']);
  assert.equal(ci[id].steps.at(-1).run, 'node .github/macos-result.mjs');
 }
});

test('unknown and forged PRs route through the actual graph to the Mac proof', () => {
 const h = history();
 try {
  for (const p of [{}, payload(h, 'version', false, false), payload(h, 'dependency')]) {
   const decision = classify(h, p);
   const runs = resolveRuns({ id: 'unknown-or-forged', event: 'pull_request', action: 'synchronize', payload: p, ref: 'refs/heads/master', macosRequired: decision.macosRequired });
   assert.ok(runs.flatMap(r => r.jobs).some(j => j.macos));
  }
 } finally { h.cleanup(); }
});


test('carrier expressions and actual outcome handling never hide failed required work', () => {
 const ci = workflows()['ci.yml'].jobs;
 for (const id of ['check-macos', 'build-macos']) for (const result of ['success', 'failure', 'cancelled', 'skipped', '']) for (const required of [true, false]) {
  const context = { needs: { policy: { result: 'success', outputs: { macosRequired: String(required) } }, macos: { result }, check: { result: 'success' } } };
  assert.equal(evaluate(ci[id].if, context).value, true);
  const step = ci[id].steps.at(-1);
  const env = Object.fromEntries(Object.entries(step.env).map(([key, value]) => [key, interpolate(value, context)]));
  const run = spawnSync('bash', ['-c', step.run], { cwd: root, env: { ...process.env, ...env }, encoding: 'utf8' });
  assert.equal(run.status === 0, required ? result === 'success' : result === 'skipped');
 }
});
