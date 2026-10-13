// Slow builder journey stays on the existing scripts/installer edge, outside
// github-workflows' cheap telemetry test. No forge or registry is contacted.
import { expect, test } from 'bun:test';
import { execFileSync, spawnSync } from 'node:child_process';
import { cpSync, mkdtempSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync, readdirSync, chmodSync, existsSync } from 'node:fs';
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
    // llmlint: ignore[work_goes_through_command_surface] This task explicitly requires the dry run to execute the edited release workflow's exact production builder command; just release is a development wrapper, whereas this journey reconciles the actual release boundary without publishing.
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
    // Only Actions artifact transport and forge/registry transport are doubled.
    // The producer's actual path list and consumer's actual pattern drive copying.
    const producer = workflow.jobs.build.steps.find(s => s.uses?.startsWith('actions/upload-artifact@'));
    const consumer = workflow.jobs.upload.steps.find(s => s.uses?.startsWith('actions/download-artifact@'));
    const artifactName = replace(producer.with.name);
    const matches = (pattern, value) => new RegExp('^' + pattern.replaceAll('.', '\\.').replaceAll('*', '.*') + '$').test(value);
    expect(matches(consumer.with.pattern, artifactName)).toBe(true);
    const transport = join(stage, 'transport', artifactName), downloaded = join(stage, 'downloaded');
    mkdirSync(transport, { recursive: true }); mkdirSync(downloaded);
    const patterns = producer.with.path.trim().split('\n');
    const assets = readdirSync(stage).filter(file => patterns.some(pattern => matches(pattern, file)));
    expect(assets.sort()).toEqual([name + '.tar.gz', name + '.tar.gz.sha256'].sort());
    for (const file of assets) { cpSync(join(stage, file), join(transport, file)); cpSync(join(transport, file), join(downloaded, file)); expect(hash(join(downloaded, file))).toBe(hash(join(stage, file))); }
    const doubles = join(stage, 'transport-bin'); mkdirSync(doubles);
    writeFileSync(join(doubles, 'gh'), '#!/bin/sh\nprintf "%s\\n" "$@" > "$CAPTURE"\n'); chmodSync(join(doubles, 'gh'), 0o755);
    const attach = workflow.jobs.upload.steps.find(step => step.name === 'Attach assets to the release');
    const capture = join(stage, 'forge-args');
    execFileSync('bash', ['-e', '-c', attach.run], { cwd: downloaded, env: { ...process.env, PATH: doubles + ':' + process.env.PATH, TAG: tag, GITHUB_REPOSITORY: 'fixture/repo', CAPTURE: capture } });
    expect(readFileSync(capture, 'utf8').trim().split('\n')).toEqual(['release', 'upload', tag, ...assets.sort(), '--clobber', '--repo', 'fixture/repo']);
    writeFileSync(join(doubles, 'cargo'), '#!/bin/sh\nprintf "%s\\n" "$@" > "$CAPTURE"\n'); chmodSync(join(doubles, 'cargo'), 0o755);
    const publish = workflow.jobs['crates-io'].steps.find(step => step.name === 'Publish (if configured)');
    for (const configured of [false, true]) {
      const registryCapture = join(stage, configured ? 'configured' : 'absent');
      execFileSync('bash', ['-e', '-c', publish.run], { cwd: downloaded, env: { ...process.env, PATH: doubles + ':' + process.env.PATH, CARGO_REGISTRY_TOKEN: configured ? 'transport-fixture' : '', CAPTURE: registryCapture } });
      expect(existsSync(registryCapture)).toBe(configured);
      if (configured) expect(readFileSync(registryCapture, 'utf8').trim()).toBe('publish\n--locked');
    }
    writeFileSync(join(stage, name + '.tar.gz.sha256'), '0'.repeat(64) + `  ${name}.tar.gz\n`);
    const failed = spawnSync('bash', ['-e', '-c', replace(workflow.jobs.build.steps.find(s => s.name === 'Install staged native archive').run)], { cwd: stage, env: { ...process.env, VERSION: tag, RUNNER_TEMP: stage }, encoding: 'utf8' });
    expect(failed.status).not.toBe(0); expect(failed.stderr).toContain('checksum');
  } finally { rmSync(stage, { recursive: true, force: true }); }
}, 600000);
