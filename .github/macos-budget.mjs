// Exit 0: report valid, current telemetry; exit 1: invalid argv/telemetry or I/O.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { report } from '@onebudgetspec/sdk';
import { digest } from './workflow-graph.mjs';
import { options } from './inputs.mjs';
const opts = options(process.argv.slice(2), ['--metric', '--telemetry']);
const telemetry = JSON.parse(readFileSync(resolve(opts['--telemetry']), 'utf8'));
const runsValid = runs => Array.isArray(runs) && runs.every(r => typeof r.workflow === 'string' && typeof r.event === 'string' && Array.isArray(r.jobs) && r.jobs.every(j => typeof j.macos === 'boolean' && typeof j.enabled === 'boolean' && typeof j.runner === 'string' && typeof j.target === 'string' && typeof j.job === 'string' && j.macos === (j.enabled && j.runner.startsWith('macos-'))));
if (telemetry.schemaVersion !== 1 || telemetry.sourceDigest !== digest()) throw new Error('missing/stale telemetry: run github-workflows:test for this tree');
if (!Array.isArray(telemetry.scenarios) || !telemetry.scenarios.length || !telemetry.scenarios.every(s => typeof s.id === 'string' && runsValid(s.runs)) || !Array.isArray(telemetry.lifecycles) || !telemetry.lifecycles.length || !telemetry.lifecycles.every(l => typeof l.ownership === 'string' && typeof l.registryCredential === 'string' && runsValid(l.runs) && l.runs.length)) throw new Error('invalid telemetry collections: run github-workflows:test');
const metric = opts['--metric'];
if (!['jobs-per-run', 'runs-per-change'].includes(metric)) throw new Error('unknown metric: use jobs-per-run or runs-per-change');
const counts = telemetry.scenarios.flatMap(s => s.runs.map(r => r.jobs.filter(j => j.macos).length));
if (!counts.length) throw new Error('empty workflow graph: regenerate telemetry');
const measured = metric === 'jobs-per-run' ? Math.max(...counts) : Math.max(...telemetry.lifecycles.map(l => l.runs.filter(r => r.jobs.some(j => j.macos)).length));
const detail = JSON.stringify({ scenarios: telemetry.scenarios.map(s => ({ event: s.id, runs: s.runs.filter(r => r.jobs.some(j => j.runner.startsWith('macos-'))).map(r => ({ workflow: r.workflow, jobs: r.jobs.filter(j => j.runner.startsWith('macos-')).map(({ job, target, enabled }) => ({ job, target, enabled })) })) })), lifecycles: telemetry.lifecycles.map(l => ({ ownership: l.ownership, registryCredential: l.registryCredential, macosRuns: l.runs.filter(r => r.jobs.some(j => j.macos)).map(r => ({ workflow: r.workflow, event: r.event })) })) });
report(measured, detail);
console.log(`${metric}: ${measured}`);
