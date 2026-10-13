import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { writeFileSync, readFileSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { releaseBranch } from '../macos-policy.mjs';
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

test('route subprocess validates classifier output and only permits explicit manual diagnostic', () => {
 const h = history();
 try {
  for (const [event, p, diagnostic, expected] of [
   ['pull_request', payload(h, 'source'), 'false', true],
   ['pull_request', payload(h, 'version'), 'false', false],
   ['workflow_dispatch', {}, 'false', false],
   ['workflow_dispatch', {}, 'true', true],
   ['push', {}, 'true', false],
  ]) {
   const policy = join(h.cwd, '.git/policy.json'), output = join(h.cwd, '.git/output');
   writeFileSync(policy, JSON.stringify(classify(h, p, event))); writeFileSync(output, '');
   const r = spawnSync(process.execPath, [join(root, '.github/macos-route.mjs'), policy], { env: { ...process.env, GITHUB_EVENT_NAME: event, DIAGNOSTIC: diagnostic, GITHUB_OUTPUT: output } });
   assert.equal(r.status, 0); assert.equal(readFileSync(output, 'utf8'), `macosRequired=${expected}\n`);
  }
  const policy = join(h.cwd, '.git/policy.json'); writeFileSync(policy, '{"macosRequired":"false"}');
  assert.notEqual(spawnSync(process.execPath, [join(root, '.github/macos-route.mjs'), policy]).status, 0);
 } finally { h.cleanup(); }
});
test('semantic classifier rejects empty, deleted, renamed, malformed and mixed contracts', () => {
 const h = history();
 try {
  h.heads.version = h.base; assert.equal(classify(h, payload(h, 'version')).macosRequired, true);
  const cases = [
   () => h.git('rm', 'Cargo.toml'),
   () => h.git('mv', 'Cargo.toml', 'Renamed.toml'),
   () => writeFileSync(join(h.cwd, 'Cargo.toml'), '[package]\nname="other"\nversion="1.1.0"\n'),
   () => writeFileSync(join(h.cwd, 'Cargo.toml'), '[package]\nname="gh-secrets"\nversion=123\n'),
   () => writeFileSync(join(h.cwd, 'Cargo.toml'), '[package]\nname="gh-secrets"\n'),
   ...['1.2.3-01', '1.2.3-.', '1.2.3+a..b'].map(v => () => writeFileSync(join(h.cwd, 'Cargo.toml'), `[package]\nname="gh-secrets"\nversion="${v}"\n`)),
   () => writeFileSync(join(h.cwd, 'Cargo.toml'), 'broken [toml'),
   () => { writeFileSync(join(h.cwd, 'Cargo.toml'), '[package]\nname="gh-secrets"\nversion="1.1.0"\nedition="2021"\n[dependencies]\nserde="1"\n'); writeFileSync(join(h.cwd, 'source.rs'), 'changed source'); },
  ];
  for (let i = 0; i < cases.length; i++) {
   h.git('checkout', '-q', '-B', `invalid-${i}`, h.base); cases[i](); h.heads.version = h.commit();
   assert.equal(classify(h, payload(h, 'version')).macosRequired, true, `case ${i}`);
  }
  const r = spawnSync(process.execPath, [join(root, '.github/macos-policy.mjs'), 'classify', '--bogus', 'value']);
  assert.notEqual(r.status, 0);
 } finally { h.cleanup(); }
});


test('unsupported release configuration is explicitly refused', () => {
 const h = history();
 try {
  mkdirSync(join(h.cwd, '.github/workflows'), { recursive: true });
  writeFileSync(join(h.cwd, 'release-please-config.json'), '{"packages":{".":{"release-type":"rust"}}}');
  for (const branches of ['master', [], ['master', 'other'], [123]]) {
   writeFileSync(join(h.cwd, '.github/workflows/release.yml'), JSON.stringify({ on: { push: { branches } } }));
   assert.throws(() => releaseBranch(h.cwd + '/'), /unsupported release configuration/);
  }
 } finally { h.cleanup(); }
});


test('member manifests and release metadata round-trip only version changes', () => {
 const h = history();
 try {
  h.git('checkout', '-q', '-B', 'member-base', h.base);
  mkdirSync(join(h.cwd, 'member')); writeFileSync(join(h.cwd, 'member/Cargo.toml'), '[package]\nname="member"\nversion="1.0.0"\nedition="2021"\n');
  h.base = h.commit();
  writeFileSync(join(h.cwd, 'member/Cargo.toml'), '[package]\nname="member"\nversion="1.1.0"\nedition="2021"\n');
  h.heads.version = h.commit(); assert.equal(classify(h, payload(h, 'version')).isReleaseOnly, true);
  for (const metadata of ['{broken', '{".":123}', '{".":"1.1.0","extra":"1.1.0"}', '{".":"1.2.3-01"}']) {
   h.git('checkout', '-q', '-B', 'metadata', h.base); writeFileSync(join(h.cwd, '.release-please-manifest.json'), metadata); h.heads.version = h.commit();
   assert.equal(classify(h, payload(h, 'version')).macosRequired, true);
  }
  assert.notEqual(spawnSync(process.execPath, [join(root, '.github/macos-policy.mjs'), 'classify', '--event', join(h.cwd, '.git/event.json'), '--base', h.base, '--head', h.heads.version], { env: { ...process.env, GITHUB_EVENT_NAME: 'unrecognized' } }).status, 0);
 } finally { h.cleanup(); }
});
