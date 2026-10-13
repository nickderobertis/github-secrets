// Slow builder journey stays on the existing scripts/installer edge, outside
// github-workflows' cheap telemetry test. No forge or registry is contacted.
import { expect, test } from 'bun:test';
import { execFileSync, spawnSync } from 'node:child_process';
import { cpSync, mkdtempSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import { REPO } from './helpers.mjs';

test.skipIf(process.platform !== 'linux' || process.arch !== 'x64')('real native release commands build, pack, install and run; corrupt checksum is refused', () => {
  const workflow = Bun.YAML.parse(readFileSync(join(REPO, '.github/workflows/release.yml'), 'utf8'));
  const target = workflow.jobs.build.strategy.matrix.include.find(r => r.os === 'ubuntu-latest').target;
  const parent = process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? process.env.RUNNER_TEMP ?? join(REPO, '.github/.telemetry');
  mkdirSync(parent, { recursive: true });
  const stage = mkdtempSync(join(parent, 'release-native-'));
  const tag = `v${Bun.TOML.parse(readFileSync(join(REPO, 'Cargo.toml'), 'utf8')).package.version}`;
  const replace = text => text.replaceAll('${{ matrix.target }}', target);
  const command = (name, cwd, env = {}) => {
    const step = workflow.jobs.build.steps.find(s => s.name === name);
    return execFileSync('bash', ['-e', '-c', replace(step.run)], { cwd, env: { ...process.env, VERSION: tag, RUNNER_TEMP: stage, ...env }, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
  };
  try {
    command('Build', REPO);
    symlinkSync(join(REPO, 'target'), join(stage, 'target'), 'dir');
    cpSync(join(REPO, 'README.md'), join(stage, 'README.md')); cpSync(join(REPO, 'LICENSE'), join(stage, 'LICENSE'));
    mkdirSync(join(stage, 'scripts')); cpSync(join(REPO, 'scripts/install.sh'), join(stage, 'scripts/install.sh'));
    command('Package (unix)', stage);
    const name = `gh-secrets-${tag}-${target}`;
    const contents = execFileSync('tar', ['tzf', join(stage, name + '.tar.gz')], { encoding: 'utf8' });
    for (const file of ['gh-secrets', 'README.md', 'LICENSE']) expect(contents).toContain(`${name}/${file}`);
    command('Install staged native archive', stage);
    expect(execFileSync(join(stage, 'bin/gh-secrets'), ['--version'], { encoding: 'utf8' }).trim()).toBe(`gh-secrets ${tag.slice(1)}`);
    const hash = path => createHash('sha256').update(readFileSync(path)).digest('hex');
    expect(hash(join(stage, 'bin/gh-secrets'))).toBe(hash(join(REPO, `target/${target}/release/gh-secrets`)));
    writeFileSync(join(stage, name + '.tar.gz.sha256'), '0'.repeat(64) + `  ${name}.tar.gz\n`);
    const failed = spawnSync('bash', ['-e', '-c', replace(workflow.jobs.build.steps.find(s => s.name === 'Install staged native archive').run)], { cwd: stage, env: { ...process.env, VERSION: tag, RUNNER_TEMP: stage }, encoding: 'utf8' });
    expect(failed.status).not.toBe(0); expect(failed.stderr).toContain('checksum');
  } finally { rmSync(stage, { recursive: true, force: true }); }
}, 600000);
