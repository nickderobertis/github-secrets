// Repo-private graph proof. Unsupported expressions/calls are errors, never zero cost.
import { createHash } from 'node:crypto';
import { readFileSync, readdirSync } from 'node:fs';
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
    call: () => new data.BooleanData(name === 'always' || (name === 'success' && Object.values(context.needs ?? {}).every(n => n.result === 'success')) || Object.values(context.needs ?? {}).some(n => n.result === (name === 'failure' ? 'failure' : 'cancelled'))),
  }]));
  const tree = new Parser(new Lexer(text).lex().tokens, Object.keys(context), [...functions.values()]).parse();
  return new Evaluator(tree, value(context), functions).evaluate();
}
export function interpolate(text, context) {
  return String(text).replace(/\$\{\{(.*?)\}\}/g, (_, expr) => {
    const result = evaluate(expr, context);
    if (result.kind === data.Kind.Null) throw new Error(`unresolved expression: ${expr}`);
    return result.coerceString();
  });
}
export function matrixRows(matrix) {
  if (!matrix) return [{}];
  if (typeof matrix !== 'object') throw new Error('unresolved matrix');
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
export function workflows(dir = join(root, '.github/workflows')) {
  return Object.fromEntries(readdirSync(dir).filter(f => /\.ya?ml$/.test(f)).sort().map(f => [f, parse(readFileSync(join(dir, f), 'utf8'))]));
}
function matches(pattern, ref) { return new RegExp('^' + pattern.replace(/[.+?^${}()|[\]\\]/g, '\\$&').replaceAll('*', '.*') + '$').test(ref); }
function triggered(wf, scenario) {
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
  function jobs(file, wf, prefix = '', stack = []) {
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
      const context = { github: { event_name: scenario.event, event: scenario.payload, repository: 'nickderobertis/github-secrets', ref: scenario.ref }, inputs: { macos_diagnostic: scenario.diagnostic ?? false }, needs };
      const explicitStatus = /\b(always|success|failure|cancelled)\(/.test(job.if ?? '');
      const prerequisites = Object.values(needs).every(n => n.result === 'success');
      const enabled = (explicitStatus || prerequisites) && (!job.if || evaluate(job.if, context).value === true);
      const outputs = id === 'policy' ? { macosRequired: String(scenario.macosRequired || scenario.diagnostic === true) } : id === 'release-please' ? { release_created: String(scenario.releaseCreated ?? false), tag_name: scenario.tag ?? 'v1.0.0' } : {};
      outcomes[id] = { result: enabled ? 'success' : 'skipped', outputs };
      for (const matrix of matrixRows(job.strategy?.matrix)) {
        const ctx = { ...context, matrix };
        if (job.uses) {
          if (!job.uses.startsWith('./.github/workflows/')) throw new Error(`unresolved reusable workflow ${job.uses}`);
          const call = job.uses.split('/').at(-1);
          if (!graph[call]) throw new Error(`missing reusable workflow ${call}`);
          if (enabled) rows.push(...jobs(call, graph[call], `${prefix}${id}/`, [...stack, file]));
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
  for (const [file, wf] of Object.entries(graph)) if (triggered(wf, scenario)) runs.push({ workflow: file, event: scenario.id, jobs: jobs(file, wf) });
  return runs;
}
export function digest() {
  const hash = createHash('sha256');
  const walk = dir => { for (const file of readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
    if (['node_modules', '.telemetry'].includes(file.name)) continue;
    const path = join(dir, file.name);
    if (file.isDirectory()) walk(path); else { hash.update(path.slice(root.length)); hash.update(readFileSync(path)); }
  } };
  walk(join(root, '.github'));
  for (const path of ['scripts/ci-gate-tier.mjs', 'scripts/install.sh', 'Cargo.toml', 'rust-toolchain.toml', 'release-please-config.json']) { hash.update(path); hash.update(readFileSync(join(root, path))); }
  return hash.digest('hex');
}
