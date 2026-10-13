// Repo-private graph proof. Unsupported expressions/calls are errors, never zero cost.
import { execFileSync } from 'node:child_process';
import { truthy } from '@actions/expressions/result';
import { createHash } from 'node:crypto';
import { readFileSync, readdirSync, mkdtempSync, mkdirSync, writeFileSync, rmSync, existsSync, symlinkSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import { parse } from 'yaml';
import { Lexer, Parser, Evaluator, data } from '@actions/expressions';
export const root = fileURLToPath(new URL('../', import.meta.url));
function value(v) {
  if (v === null || v === undefined) return new data.Null();
  if (typeof v === 'boolean') return new data.BooleanData(v);
  if (typeof v === 'number') return new data.NumberData(v);
  if (typeof v === 'string') return new data.StringData(v);
  if (Array.isArray(v)) return new data.Array(...v.map(value));
  return new data.Dictionary(...Object.entries(v).map(([key, v]) => ({ key, value: value(v) })));
}
export function evaluate(expression, context) {
  const text = expression.replace(/^\s*\$\{\{\s*|\s*\}\}\s*$/g, '');
  const functions = new Map(['always', 'success', 'failure', 'cancelled'].map(name => [name, {
    name, minArgs: 0, maxArgs: 0,
    call: () => new data.BooleanData(name === 'always' || (name === 'success' && Object.values(context.needs ?? {}).every(n => n.result === 'success')) || (['failure', 'cancelled'].includes(name) && Object.values(context.needs ?? {}).some(n => n.result === name))),
  }]));
  const tree = new Parser(new Lexer(text).lex().tokens, Object.keys(context), [...functions.values()]).parse();
  return new Evaluator(tree, value(context), functions).evaluate();
}
export function interpolate(text, context, allowMissing = false) {
  return String(text).replace(/\$\{\{(.*?)\}\}/g, (_, expr) => {
    const result = evaluate(expr, context);
    if (result.kind === data.Kind.Null && allowMissing) return '';
    if (result.kind === data.Kind.Null) throw new Error(`unresolved expression: ${expr}`);
    return result.coerceString();
  });
}
export function matrixRows(matrix) {
  if (matrix === undefined) return [{}];
  if (!matrix) throw new Error('unresolved matrix');
  if (typeof matrix !== 'object' || Array.isArray(matrix)) throw new Error('unresolved matrix');
  for (const key of ['include', 'exclude']) if (key in matrix && (!Array.isArray(matrix[key]) || !matrix[key].every(e => e && typeof e === 'object' && !Array.isArray(e)))) throw new Error(`unresolved matrix ${key}`);
  let rows = [{}];
  for (const [key, values] of Object.entries(matrix).filter(([k]) => !['include', 'exclude'].includes(k))) {
    if (!Array.isArray(values)) throw new Error(`unresolved matrix axis ${key}`);
    rows = rows.flatMap(row => values.map(v => ({ ...row, [key]: v })));
  }
  const original = rows.map(r => ({ ...r }));
  rows = rows.filter(r => !(matrix.exclude ?? []).some(e => Object.entries(e).every(([k, v]) => r[k] === v)));
  for (const addition of matrix.include ?? []) {
    const compatible = original.filter(r => Object.entries(r).every(([k, v]) => !(k in addition) || addition[k] === v));
    if (!Object.keys(original[0] ?? {}).length) { if (rows.length === 1 && !Object.keys(rows[0]).length) rows = []; rows.push(addition); }
    else if (!compatible.length) rows.push(addition);
    else for (const r of rows) if (compatible.some(o => Object.entries(o).every(([k, v]) => r[k] === v))) Object.assign(r, addition);
  }
  return rows;
}
export const MODELLED_EVENTS = ['pull_request', 'push', 'release', 'workflow_run', 'repository_dispatch', 'schedule', 'workflow_dispatch', 'workflow_call'];
function validateWorkflow(file, wf) {
  const object = v => v && typeof v === 'object' && !Array.isArray(v);
  if (!object(wf) || !object(wf.jobs) || !wf.on) throw new Error(`unsupported workflow ${file}`);
  if (!object(wf.on) && typeof wf.on !== 'string' && !(Array.isArray(wf.on) && wf.on.length && wf.on.every(e => typeof e === 'string'))) throw new Error(`unsupported trigger ${file}`);
  // Only events the telemetry scenarios cover; anything else would count as no Mac work.
  for (const event of object(wf.on) ? Object.keys(wf.on) : [wf.on].flat()) if (!MODELLED_EVENTS.includes(event)) throw new Error(`unsupported trigger ${file}:${event}: add it to the modelled scenarios first`);
  if (object(wf.on)) for (const [event, filters] of Object.entries(wf.on)) {
    if (filters === null) continue;
    if (!object(filters)) throw new Error(`unsupported trigger ${file}:${event}`);
    if (['workflow_run', 'workflow_call'].includes(event) && Object.keys(filters).length) throw new Error(`unresolved event/call inputs ${file}:${event}`);
    const allowed = event === 'workflow_dispatch' ? ['inputs'] : event === 'workflow_call' ? ['inputs', 'secrets', 'outputs'] : event === 'push' ? ['branches', 'tags'] : event === 'pull_request' ? ['types'] : ['types', 'workflows'];
    if (Object.keys(filters).some(k => !allowed.includes(k))) throw new Error(`unsupported trigger filter ${file}:${event}`);
    for (const key of ['branches', 'tags', 'types', 'workflows']) if (key in filters && (!Array.isArray(filters[key]) || !filters[key].every(v => typeof v === 'string'))) throw new Error(`unsupported filter ${file}:${key}`);
    // matches() implements literal characters and `*` only; refuse other glob syntax.
    for (const key of ['branches', 'tags']) if (key in filters && !filters[key].every(v => /^[A-Za-z0-9._/*-]+$/.test(v))) throw new Error(`unsupported ${key} pattern ${file}:${event}`);
    // Only boolean dispatch inputs are modelled; their defaults must be booleans.
    if (event === 'workflow_dispatch' && 'inputs' in filters && (!object(filters.inputs) || !Object.values(filters.inputs).every(spec => object(spec) && spec.type === 'boolean' && (spec.default === undefined || typeof spec.default === 'boolean')))) throw new Error(`unsupported dispatch inputs ${file}`);
  }
  for (const [id, job] of Object.entries(wf.jobs)) {
    if (!object(job) || (job.if !== undefined && typeof job.if !== 'string') || (job.needs !== undefined && typeof job.needs !== 'string' && (!Array.isArray(job.needs) || !job.needs.every(n => typeof n === 'string')))) throw new Error(`unsupported job ${file}:${id}`);
    if (job.uses && (job.with || job.secrets || job.strategy)) throw new Error(`unresolved reusable workflow inputs ${file}:${id}`);
    if (job.steps !== undefined && (!Array.isArray(job.steps) || !job.steps.every(object))) throw new Error(`unsupported steps ${file}:${id}`);
  }
  return wf;
}
function routingOutput(scenario, job, context) {
  const parent = process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? process.env.RUNNER_TEMP ?? join(root, '.github/.telemetry');
  mkdirSync(parent, { recursive: true });
  const dir = mkdtempSync(join(parent, 'route-'));
  try {
    const event = join(dir, 'event.json'), output = join(dir, 'output');
    writeFileSync(event, JSON.stringify(scenario.payload)); writeFileSync(output, '');
    const step = job.steps.find(s => s.id === 'policy');
    if (!step?.run || !step.env) throw new Error('missing policy routing step');
    const env = Object.fromEntries(Object.entries(step.env).map(([key, value]) => [key, interpolate(value, context, true)]));
    const cwd = scenario.cwd ?? root;
    if (cwd !== root && !existsSync(join(cwd, '.github'))) symlinkSync(join(root, '.github'), join(cwd, '.github'), 'dir');
    // Run the real two-command workflow step, in the generated git history.
    // Only GitHub's event file and output-file transport are doubled.
    execFileSync('bash', ['-e', '-c', step.run], { cwd, env: { ...process.env, ...env, GITHUB_EVENT_NAME: scenario.event, GITHUB_EVENT_PATH: event, GITHUB_OUTPUT: output } });
    const text = readFileSync(output, 'utf8').trim();
    if (!/^macosRequired=(true|false)$/.test(text)) throw new Error('invalid route output');
    return Object.fromEntries(text.split('\n').map(line => line.split('=')));
  } finally { rmSync(dir, { recursive: true, force: true }); }
}
export function workflows(dir = join(root, '.github/workflows')) {
  return Object.fromEntries(readdirSync(dir).filter(f => /\.ya?ml$/.test(f)).sort().map(f => [f, validateWorkflow(f, parse(readFileSync(join(dir, f), 'utf8')))]));
}
function matches(pattern, ref) { return new RegExp('^' + pattern.replace(/[.+?^${}()|[\]\\]/g, '\\$&').replaceAll('*', '.*') + '$').test(ref); }
function triggered(file, wf, scenario) {
  // A manual dispatch names exactly one workflow; other events fan out.
  if (scenario.event === 'workflow_dispatch') {
    if (typeof scenario.workflow !== 'string') throw new Error('workflow_dispatch scenario must name its target workflow');
    if (scenario.workflow !== file) return false;
  }
  const on = wf.on;
  if (typeof on === 'string') return on === scenario.event;
  if (Array.isArray(on)) return on.includes(scenario.event);
  if (!(scenario.event in on)) return false;
  const filters = on[scenario.event] ?? {};
  if (filters.paths || filters['paths-ignore']) throw new Error('unresolved event path filter');
  if (filters.types && !filters.types.includes(scenario.action)) return false;
  if (scenario.event === 'push') {
    const tag = scenario.ref.startsWith('refs/tags/');
    const patterns = tag ? filters.tags : filters.branches;
    if (!patterns && (filters.tags || filters.branches)) return false;
    if (patterns && !patterns.some(p => matches(p, scenario.ref.replace(/^refs\/(heads|tags)\//, '')))) return false;
  }
  return true;
}
export function resolveRuns(scenario, graph = workflows()) {
  const runs = [];
  for (const [file, wf] of Object.entries(graph)) validateWorkflow(file, wf);
  // As Actions does: only a workflow_dispatch carries inputs (declared defaults
  // overlaid by the dispatch payload); callees here take no `with:` inputs.
  function inputsFor(wf) {
    if (scenario.event !== 'workflow_dispatch') return {};
    const declared = (typeof wf.on === 'object' && !Array.isArray(wf.on) ? wf.on.workflow_dispatch?.inputs : null) ?? {};
    const supplied = scenario.payload?.inputs ?? {};
    if (!supplied || typeof supplied !== 'object' || Array.isArray(supplied)) throw new Error('unsupported dispatch payload inputs');
    for (const [key, value] of Object.entries(supplied)) if (!(key in declared) || typeof value !== 'boolean') throw new Error(`unsupported dispatch input ${key}`);
    return { ...Object.fromEntries(Object.entries(declared).map(([key, spec]) => [key, spec.default ?? false])), ...supplied };
  }
  function jobs(file, wf, prefix = '', stack = [], inputs = inputsFor(wf)) {
    if (stack.includes(file)) throw new Error(`recursive workflow call ${file}`);
    const rows = [];
    const outcomes = {};
    const ordered = [], visiting = new Set(), visited = new Set();
    function visit(id) {
      if (visited.has(id)) return;
      if (visiting.has(id) || !wf.jobs[id]) throw new Error(`unresolved/cyclic job ${file}:${id}`);
      visiting.add(id);
      for (const need of [wf.jobs[id].needs ?? []].flat()) visit(need);
      visiting.delete(id); visited.add(id); ordered.push([id, wf.jobs[id]]);
    }
    for (const id of Object.keys(wf.jobs)) visit(id);
    for (const [id, job] of ordered) {
      const needs = Object.fromEntries([job.needs ?? []].flat().map(n => {
        if (!outcomes[n]) throw new Error(`${file}:${id}: unresolved needs ${n}`);
        return [n, outcomes[n]];
      }));
      const context = { github: { event_name: scenario.event, event: scenario.payload, repository: 'nickderobertis/github-secrets', ref: scenario.ref }, inputs, needs };
      const explicitStatus = /\b(always|success|failure|cancelled)\(/.test(job.if ?? '');
      const prerequisites = Object.values(needs).every(n => n.result === 'success');
      const enabled = (explicitStatus || prerequisites) && (!job.if || truthy(evaluate(job.if, context)));
      const steps = id === 'policy' ? { policy: { outputs: enabled ? routingOutput(scenario, job, context) : {} } } : id === 'release-please' ? { release: { outputs: { release_created: String(scenario.releaseCreated ?? false), tag_name: scenario.tag ?? 'v1.0.0' } } } : {};
      const outputs = Object.fromEntries(Object.entries(job.outputs ?? {}).map(([key, expression]) => [key, interpolate(expression, { ...context, steps })]));
      outcomes[id] = { result: enabled ? 'success' : 'skipped', outputs };
      for (const matrix of matrixRows(job.strategy?.matrix)) {
        const ctx = { ...context, matrix };
        if (job.uses) {
          if (!job.uses.startsWith('./.github/workflows/')) throw new Error(`unresolved reusable workflow ${job.uses}`);
          const call = job.uses.split('/').at(-1);
          if (!graph[call]) throw new Error(`missing reusable workflow ${call}`);
          rows.push(...jobs(call, graph[call], `${prefix}${id}/`, [...stack, file], {}).map(row => ({ ...row, enabled: enabled && row.enabled, macos: enabled && row.macos })));
          continue;
        }
        if (!job['runs-on']) throw new Error(`unresolved runner ${file}:${id}`);
        const runner = interpolate(job['runs-on'], ctx);
        if (!/^(ubuntu|macos|windows)-[\w.-]+$/.test(runner)) throw new Error(`unsupported runner ${runner}`);
        const name = job.name ? interpolate(job.name, ctx) : id + (Object.keys(matrix).length ? ` (${Object.values(matrix).join(', ')})` : '');
        rows.push({ workflow: file, event: scenario.id, job: prefix + id, name, runner, target: matrix.target ?? 'host', enabled, macos: enabled && runner.startsWith('macos-') });
      }
    }
    return rows;
  }
  for (const [file, wf] of Object.entries(graph)) if (triggered(file, wf, scenario)) runs.push({ workflow: file, event: scenario.id, jobs: jobs(file, wf) });
  return runs;
}
export function digest() {
  const hash = createHash('sha256');
  // Include the full source tree, including newly added source before staging.
  // Git's exclusions keep build products, credentials and telemetry out.
  const files = execFileSync('git', ['ls-files', '-z', '--cached', '--others', '--exclude-standard'], { cwd: root, encoding: 'utf8' }).split('\0').filter(Boolean).sort();
  for (const file of files) { hash.update(file); hash.update(readFileSync(join(root, file))); }
  return hash.digest('hex');
}
