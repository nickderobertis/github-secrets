import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { writeFileSync, readFileSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { releaseBranchPrefix } from '../macos-policy.mjs';
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
  for (const check of ['failure', 'cancelled', 'skipped', '']) {
   const r = spawnSync(process.execPath, [join(root, '.github/macos-result.mjs')], { encoding: 'utf8', env: { ...process.env, POLICY_RESULT: 'success', CHECK_RESULT: check, MACOS_REQUIRED: 'true', MACOS_RESULT: 'success' } });
   assert.notEqual(r.status, 0); assert.match(r.stderr, new RegExp(`check=${check || 'missing'}`));
  }
  const env = { ...process.env, POLICY_RESULT: 'success', MACOS_REQUIRED: 'true', MACOS_RESULT: 'success' }; delete env.CHECK_RESULT;
  assert.equal(spawnSync(process.execPath, [join(root, '.github/macos-result.mjs')], { env }).status, 0);
});

test('route subprocess validates classifier output and routes on it alone', () => {
 const h = history();
 try {
  for (const [event, p, expected] of [
   ['pull_request', payload(h, 'source'), true],
   ['pull_request', payload(h, 'version'), false],
   ['push', {}, false],
   ['workflow_dispatch', { inputs: { macos_diagnostic: true } }, false],
  ]) {
   const policy = join(h.cwd, '.git/policy.json'), output = join(h.cwd, '.git/output');
   writeFileSync(policy, JSON.stringify(classify(h, p, event))); writeFileSync(output, '');
   // An automatic caller's opt-in variable carries no authority here.
   const r = spawnSync(process.execPath, [join(root, '.github/macos-route.mjs'), policy], { env: { ...process.env, GITHUB_EVENT_NAME: event, DIAGNOSTIC: 'true', GITHUB_OUTPUT: output } });
   assert.equal(r.status, 0); assert.equal(readFileSync(output, 'utf8'), `macosRequired=${expected}\n`);
  }
  const policy = join(h.cwd, '.git/policy.json'); writeFileSync(policy, '{"macosRequired":"false"}');
  assert.notEqual(spawnSync(process.execPath, [join(root, '.github/macos-route.mjs'), policy]).status, 0);
  writeFileSync(policy, JSON.stringify(classify(h, payload(h, 'source'))));
  for (const output of [undefined, '', join(h.cwd, '.git/missing-dir/output')]) {
   const env = { ...process.env, GITHUB_EVENT_NAME: 'pull_request' }; if (output === undefined) delete env.GITHUB_OUTPUT; else env.GITHUB_OUTPUT = output;
   const r = spawnSync(process.execPath, [join(root, '.github/macos-route.mjs'), policy], { env, encoding: 'utf8' });
   assert.notEqual(r.status, 0); assert.match(r.stderr, /GITHUB_OUTPUT must name/);
  }
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
   assert.throws(() => releaseBranchPrefix(h.cwd + '/'), /unsupported release configuration/);
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
 } finally { h.cleanup(); }
});

test('unrecognized event names require Mac and name the event; known non-PR events stay Mac-free', () => {
 const h = history();
 try {
  for (const event of ['merge_group', 'pull_request_target']) {
   const decision = classify(h, payload(h, 'version'), event);
   assert.equal(decision.macosRequired, true);
   assert.match(decision.reason, new RegExp(`unrecognized event "${event}"`));
  }
  for (const event of ['push', 'release', 'workflow_run', 'repository_dispatch', 'schedule', 'workflow_dispatch']) {
   const decision = classify(h, payload(h, 'version'), event);
   assert.equal(decision.macosRequired, false, event);
   assert.match(decision.reason, new RegExp(`non-PR event ${event}`));
  }
 } finally { h.cleanup(); }
});
test('release-only is decided by a same-repository head under the release prefix and the diff alone', () => {
 const h = history();
 try {
  const withRef = (ref, fork = false) => { const p = payload(h, 'version', fork); p.pull_request.head.ref = ref; return p; };
  assert.equal(classify(h, withRef('release-please--branches--master')).isReleaseOnly, true);
  assert.equal(classify(h, withRef('release-please--branches--master--components--other')).isReleaseOnly, true);
  for (const ref of ['release-please--branches--masterx', 'feature/release-please--branches--master', 'release-please--branches--main']) {
   const decision = classify(h, withRef(ref));
   assert.equal(decision.macosRequired, true, ref); assert.match(decision.reason, /not under release-please--branches--master/);
  }
  const fork = classify(h, withRef('release-please--branches--master--components--gh-secrets', true));
  assert.equal(fork.macosRequired, true); assert.match(fork.reason, /not a same-repository head/);
  // A bot author and release title on a feature branch carry no authority.
  const bot = payload(h, 'version', false, false); bot.pull_request.user.login = 'release-please[bot]';
  assert.equal(classify(h, bot).macosRequired, true);
  // The base advancing after the release branch was cut does not leak into its diff.
  h.git('checkout', '-q', '-B', 'advanced', h.base); writeFileSync(join(h.cwd, 'later.rs'), 'fn later() {}');
  const advanced = h.commit(); const p = payload(h, 'version'); p.pull_request.base.sha = advanced;
  const file = join(h.cwd, '.git/event.json'); writeFileSync(file, JSON.stringify(p));
  const r = spawnSync(process.execPath, [join(root, '.github/macos-policy.mjs'), 'classify', '--event', file, '--base', advanced, '--head', h.heads.version], { cwd: h.cwd, encoding: 'utf8', env: { ...process.env, GITHUB_EVENT_NAME: 'pull_request' } });
  assert.equal(JSON.parse(r.stdout).isReleaseOnly, true);
 } finally { h.cleanup(); }
});
test('a registry dependency sharing the local package name is a dependency change, not a release bump', () => {
 const h = history();
 try {
  const lock = (local, registry) => `version = 4\n[[package]]\nname = "gh-secrets"\nversion = "${local}"\n[[package]]\nname = "gh-secrets"\nversion = "${registry}"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n`;
  h.git('checkout', '-q', '-B', 'same-name-base', h.base); writeFileSync(join(h.cwd, 'Cargo.lock'), lock('1.0.0', '0.9.0')); h.base = h.commit();
  writeFileSync(join(h.cwd, 'Cargo.lock'), lock('1.0.0', '0.9.1')); h.heads.version = h.commit();
  const decision = classify(h, payload(h, 'version'));
  assert.equal(decision.macosRequired, true); assert.match(decision.reason, /Cargo.lock changes more than its own version/);
  h.git('checkout', '-q', '-B', 'same-name-local', h.base); writeFileSync(join(h.cwd, 'Cargo.lock'), lock('1.1.0', '0.9.0')); h.heads.version = h.commit();
  assert.equal(classify(h, payload(h, 'version')).isReleaseOnly, true);
 } finally { h.cleanup(); }
});
test('non-string or malformed repository identities never authorize omission', () => {
 const h = history();
 try {
  for (const name of [1, true, ['a/b'], { full_name: 'x' }, '', 'no-slash']) {
   const p = payload(h, 'version'); p.pull_request.head.repo.full_name = name; p.pull_request.base.repo.full_name = name;
   const decision = classify(h, p);
   assert.equal(decision.macosRequired, true, JSON.stringify(name)); assert.match(decision.reason, /not a same-repository head/);
  }
 } finally { h.cleanup(); }
});
test('changelog-only and valid prerelease/build-metadata release diffs are release-only', () => {
 const h = history();
 try {
  h.git('checkout', '-q', '-B', 'changelog-only', h.base); writeFileSync(join(h.cwd, 'CHANGELOG.md'), '# Changelog\nNotes only\n'); h.heads.version = h.commit();
  assert.equal(classify(h, payload(h, 'version')).isReleaseOnly, true);
  for (const v of ['1.1.0-rc.1', '1.1.0-alpha.0+build.5', '2.0.0+20261013']) {
   h.git('checkout', '-q', '-B', `pre-${v}`, h.base);
   writeFileSync(join(h.cwd, 'Cargo.toml'), `[package]\nname = "gh-secrets"\nversion = "${v}"\nedition = "2021"\n[dependencies]\nserde = "1"\n`);
   writeFileSync(join(h.cwd, '.release-please-manifest.json'), JSON.stringify({ '.': v }));
   h.heads.version = h.commit();
   assert.equal(classify(h, payload(h, 'version')).isReleaseOnly, true, v);
  }
 } finally { h.cleanup(); }
});
