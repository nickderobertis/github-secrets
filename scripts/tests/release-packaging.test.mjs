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
    // llmlint: ignore[work_goes_through_command_surface] the release boundary under test is release.yml's own Build command (cargo, no just), so the journey runs that step verbatim rather than the `just release` dev wrapper.
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
    const matches = (pattern, value) => new RegExp('^' + pattern.replaceAll('.', '\\.').replaceAll('*', '.*') + '$').test(value);
    const patterns = producer.with.path.trim().split('\n');
    const downloaded = join(stage, 'downloaded'); mkdirSync(downloaded);
    const expected = [];
    // Every release target crosses the same handoff. The host target carries the
    // archive built above; the others (whose pwsh/Darwin packaging this Linux
    // host cannot run) carry named placeholders, proving their .tar.gz/.zip and
    // .sha256 names are selected, transported and attached, not their bytes.
    for (const row of workflow.jobs.build.strategy.matrix.include) {
      const produced = join(stage, 'produced', row.target); mkdirSync(produced, { recursive: true });
      const asset = `gh-secrets-${tag}-${row.target}.${row.archive}`;
      for (const file of [asset, asset + '.sha256']) {
        if (row.target === target) cpSync(join(stage, file), join(produced, file));
        else writeFileSync(join(produced, file), `placeholder ${file}\n`);
      }
      writeFileSync(join(produced, 'unrelated.log'), 'not an asset');
      const artifactName = producer.with.name.replaceAll('${{ matrix.target }}', row.target);
      expect(matches(consumer.with.pattern, artifactName)).toBe(true);
      const selected = readdirSync(produced).filter(file => patterns.some(pattern => matches(pattern, file)));
      expect(selected.sort()).toEqual([asset, asset + '.sha256'].sort());
      for (const file of selected) { cpSync(join(produced, file), join(downloaded, file)); expect(hash(join(downloaded, file))).toBe(hash(join(produced, file))); }
      expected.push(asset, asset + '.sha256');
    }
    expect(expected.filter(f => f.endsWith('.zip'))).toEqual([`gh-secrets-${tag}-x86_64-pc-windows-msvc.zip`]);
    const doubles = join(stage, 'transport-bin'); mkdirSync(doubles);
    writeFileSync(join(doubles, 'gh'), '#!/bin/sh\nprintf "%s\\n" "$@" > "$CAPTURE"\n'); chmodSync(join(doubles, 'gh'), 0o755);
    const attach = workflow.jobs.upload.steps.find(step => step.name === 'Attach assets to the release');
    const capture = join(stage, 'forge-args');
    execFileSync('bash', ['-e', '-c', attach.run], { cwd: downloaded, env: { ...process.env, PATH: doubles + ':' + process.env.PATH, TAG: tag, GITHUB_REPOSITORY: 'fixture/repo', CAPTURE: capture } });
    const args = readFileSync(capture, 'utf8').trim().split('\n');
    expect(args.slice(0, 3)).toEqual(['release', 'upload', tag]);
    expect(args.slice(-3)).toEqual(['--clobber', '--repo', 'fixture/repo']);
    expect(args.slice(3, -3).sort()).toEqual(expected.sort());
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
