import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { root } from '../workflow-graph.mjs';
import { history, payload, classify } from './helpers.mjs';
test('classifier drives real git diffs, fails closed, and ignores author/title authority', () => {
  const h = history();
  try {
    assert.equal(classify(h, payload(h, 'version')).isReleaseOnly, true);
    for (const kind of ['source', 'dependency']) assert.equal(classify(h, payload(h, kind)).macosRequired, true);
    assert.equal(classify(h, payload(h, 'version', true)).macosRequired, true);
    const forged = payload(h, 'version', false, false); forged.pull_request.user.login = 'github-actions[bot]';
    assert.equal(classify(h, forged).macosRequired, true);
    assert.equal(classify(h, '{broken').macosRequired, true);
    assert.equal(classify(h, payload(h, 'version'), 'pull_request', 'unreadable-ref').macosRequired, true);
    assert.equal(classify(h, '{broken', 'push').macosRequired, false);
    h.git('checkout', '-q', '-B', 'lock-edit', h.base);
    writeFileSync(join(h.cwd, 'Cargo.lock'), 'version = 4\n[[package]]\nname = "gh-secrets"\nversion = "1.1.0"\n[[package]]\nname = "serde"\nversion = "1.0.0"\nsource = "registry"\n');
    h.heads.version = h.commit(); assert.equal(classify(h, payload(h, 'version')).isReleaseOnly, true);
    writeFileSync(join(h.cwd, 'Cargo.lock'), 'version = 4\n[[package]]\nname = "serde"\nversion = "2.0.0"\nsource = "registry"\n');
    h.heads.version = h.commit(); assert.equal(classify(h, payload(h, 'version')).macosRequired, true);
  } finally { h.cleanup(); }
});
test('real carrier command accepts only success or classifier-authorized omission', () => {
  for (const required of ['true', 'false', '', 'garbage']) for (const mac of ['success', 'failure', 'cancelled', 'skipped', '']) for (const prerequisite of ['success', 'failure', 'cancelled', 'skipped', '']) {
    const r = spawnSync(process.execPath, [join(root, '.github/macos-result.mjs')], { encoding: 'utf8', env: { ...process.env, POLICY_RESULT: prerequisite, CHECK_RESULT: 'success', MACOS_REQUIRED: required, MACOS_RESULT: mac } });
    const accepted = prerequisite === 'success' && ((required === 'true' && mac === 'success') || (required === 'false' && mac === 'skipped'));
    assert.equal(r.status === 0, accepted, `${required}/${mac}/${prerequisite}`);
    if (accepted && required === 'false') assert.match(r.stdout, /no Mac assertions ran/);
  }
  const r = spawnSync(process.execPath, [join(root, '.github/macos-result.mjs')], { env: { ...process.env, POLICY_RESULT: 'success', CHECK_RESULT: 'failure', MACOS_REQUIRED: 'true', MACOS_RESULT: 'success' } });
  assert.notEqual(r.status, 0);
});
