// The affected merge path, asked of the real Nx graph: a classifier-only change
// must select a target that executes scripts' tier-router journeys. Plugins load
// in-process: this Nx runs inside an Nx task, where a loaded Windows runner
// timed out the isolated plugin worker's 10-second handshake.
import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { root } from '../workflow-graph.mjs';
const nx = (...args) => JSON.parse(execFileSync('bash', ['scripts/nx', ...args], { cwd: root, encoding: 'utf8', env: { ...process.env, NX_DAEMON: 'false', NX_ISOLATE_PLUGINS: 'false' } }).trim().split('\n').at(-1));
test('a classifier-only change selects github-workflows, whose test runs the ci-gate-tier journeys and not the packaging suite', () => {
  for (const file of ['.github/macos-policy.mjs', '.github/inputs.mjs']) {
    const affected = nx('show', 'projects', '--affected', `--files=${file}`, '--json');
    assert.ok(affected.includes('github-workflows'), `${file}: ${affected}`);
  }
  const project = nx('show', 'project', 'github-workflows', '--json');
  const command = project.targets.test.options.command;
  assert.match(command, /(^|&& )bun test [^&]*scripts\/tests\/ci-gate-tier\.test\.mjs(\s|$)/);
  assert.doesNotMatch(command, /release-packaging|bun test [^&]*scripts\/tests(\s|$)/);
  // The wiring adds no reverse edge, so the graph stays acyclic.
  assert.ok(!(nx('show', 'project', 'scripts', '--json').implicitDependencies ?? []).includes('github-workflows'));
});
