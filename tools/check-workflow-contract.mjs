// The CI contract the rest of the repo (and branch protection) relies on, read
// from the committed workflow files:
//
//   1. Fixed status-check contexts. Each name below is reported by exactly one
//      job of a workflow that runs on every pull request (no branch or path
//      filter on its `pull_request` trigger); that job's `if:` is one of the
//      conditions known to keep it reporting on a pull request, and it `needs`
//      only other fixed-context jobs — so nothing can silently leave a context
//      unreported. notignored, by contrast, must stay out of that set: its own
//      workflow, needed by no fixed-context job.
//   2. ci.yml defaults the token to `contents: read`, and every ci.yml job
//      installs `just` at the version .tool-versions pins.
//   3. CI's install-path job packages the release archive with the SAME steps
//      release.yml uses, so the archive install.sh is proven against is the one
//      a release ships.
//   4. rust-toolchain.toml's `targets` are exactly release.yml's build matrix
//      targets, so the pinned toolchain always carries what a release builds.
//
// Usage: bun tools/check-workflow-contract.mjs [--root <dir>]
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";

export const FIXED_CONTEXTS = [
  "check (ubuntu-latest)",
  "check (macos-latest)",
  "check (windows-latest)",
  "build (ubuntu-latest)",
  "build (macos-latest)",
  "build (windows-latest)",
  "live-e2e",
  "live-e2e-bitwarden",
  "llmlint",
  "lint PR title",
];

// `if:` conditions under which a job still reports on a same-repo pull request.
// (The live jobs' condition predates this contract: a skipped job still reports.)
const PR_SAFE_CONDITIONS = new Set([
  undefined,
  "github.event_name == 'pull_request'",
  "${{ github.event_name == 'push' || github.event.pull_request.head.repo.full_name == github.repository }}",
]);

const PIN_STEP = "just@${{ steps.pins.outputs.just }}";

/** The contexts a job reports: `name` or job id, expanded over an `os` matrix. */
function contextsOf(id, job) {
  const base = job.name ?? id;
  const oses = job.strategy?.matrix?.os ?? job.strategy?.matrix?.include?.map((i) => i.os);
  if (!oses) return [base];
  if (job.name) return oses.map((os) => base.replace("${{ matrix.os }}", os));
  return oses.map((os) => `${id} (${os})`);
}

function triggersOnEveryPr(on) {
  if (on === "pull_request") return true;
  if (Array.isArray(on)) return on.includes("pull_request");
  if (!on || typeof on !== "object" || !("pull_request" in on)) return false;
  const pr = on.pull_request ?? {};
  return !pr.branches && !pr["branches-ignore"] && !pr.paths && !pr["paths-ignore"];
}

const isObject = (v) => v !== null && typeof v === "object" && !Array.isArray(v);

/** Parse every workflow, refusing shapes the checks below cannot reason about. */
function loadWorkflows(dir, errors) {
  const workflows = {};
  for (const f of readdirSync(dir).filter((f) => f.endsWith(".yml") || f.endsWith(".yaml"))) {
    let wf;
    try {
      wf = Bun.YAML.parse(readFileSync(join(dir, f), "utf8"));
    } catch (err) {
      errors.push(`${f} is not valid YAML: ${err.message}`);
      continue;
    }
    if (!isObject(wf) || !isObject(wf.jobs)) {
      errors.push(`${f} has no jobs mapping.`);
      continue;
    }
    for (const [id, job] of Object.entries(wf.jobs)) {
      const matrix = isObject(job) ? job.strategy?.matrix : undefined;
      const strings = (v) => Array.isArray(v) && v.every((x) => typeof x === "string");
      if (
        isObject(job) &&
        ((job.name !== undefined && typeof job.name !== "string") ||
          (matrix !== undefined && !isObject(matrix)) ||
          (matrix?.os !== undefined && !strings(matrix.os)) ||
          (matrix?.include !== undefined && !(Array.isArray(matrix.include) && matrix.include.every(isObject))))
      ) {
        errors.push(`${f}:${id} has a name or strategy.matrix (os / include) of an unexpected shape.`);
        wf.jobs[id] = {};
        continue;
      }
      if (!isObject(job) || (job.steps !== undefined && !Array.isArray(job.steps))) {
        errors.push(`${f}:${id} must be a mapping whose steps (if any) are a list.`);
        wf.jobs[id] = {};
      } else if ((job.steps ?? []).some((step) => !isObject(step))) {
        errors.push(`${f}:${id} has a step that is not a mapping.`);
        job.steps = job.steps.filter(isObject);
      }
    }
    workflows[f] = wf;
  }
  return workflows;
}

export function checkContract(root) {
  const errors = [];
  const workflows = loadWorkflows(join(root, ".github/workflows"), errors);
  const reporters = new Map(); // context -> [file, jobId]

  for (const [file, wf] of Object.entries(workflows)) {
    const onPr = triggersOnEveryPr(wf.on);
    for (const [id, job] of Object.entries(wf.jobs ?? {})) {
      for (const ctx of contextsOf(id, job)) {
        if (!FIXED_CONTEXTS.includes(ctx)) continue;
        if (reporters.has(ctx)) errors.push(`context '${ctx}' is reported by two jobs (${reporters.get(ctx).join(":")} and ${file}:${id}).`);
        reporters.set(ctx, [file, id]);
        if (!onPr) errors.push(`${file}:${id} reports '${ctx}' but ${file} does not run on every pull request.`);
        if (!PR_SAFE_CONDITIONS.has(job.if)) errors.push(`${file}:${id} reports '${ctx}' under an unrecognised condition: if: ${job.if}`);
      }
    }
    for (const [id, job] of Object.entries(wf.jobs ?? {})) {
      const reportsFixed = contextsOf(id, job).some((c) => FIXED_CONTEXTS.includes(c));
      if (!reportsFixed) continue;
      for (const need of [job.needs ?? []].flat()) {
        const needed = wf.jobs[need];
        const neededFixed = needed && contextsOf(need, needed).some((c) => FIXED_CONTEXTS.includes(c));
        if (!neededFixed) errors.push(`${file}:${id} reports a fixed context but needs '${need}', which reports none.`);
      }
    }
  }
  for (const ctx of FIXED_CONTEXTS) {
    if (!reporters.has(ctx)) errors.push(`no job reports the fixed context '${ctx}'.`);
  }

  // notignored: present, on pull_request, its own workflow, needed by nobody fixed.
  const notignored = workflows["notignored.yml"];
  if (!notignored) errors.push("notignored.yml is missing.");
  else {
    const usesAction = Object.values(notignored.jobs ?? {}).some((j) =>
      (j.steps ?? []).some((s) => String(s.uses ?? "").startsWith("nickderobertis/notignored@")),
    );
    if (!usesAction || !triggersOnEveryPr(notignored.on)) errors.push("notignored.yml must run nickderobertis/notignored on pull_request.");
    for (const [id, job] of Object.entries(notignored.jobs ?? {})) {
      for (const ctx of contextsOf(id, job)) {
        if (FIXED_CONTEXTS.includes(ctx)) errors.push(`notignored.yml:${id} reports the fixed context '${ctx}'.`);
      }
    }
  }

  // ci.yml: least privilege, and `just` at the pinned version in every job.
  const ci = workflows["ci.yml"];
  if (ci?.permissions?.contents !== "read" || Object.keys(ci.permissions).length !== 1) {
    errors.push("ci.yml must set top-level `permissions: contents: read` (and nothing wider).");
  }
  for (const [id, job] of Object.entries(ci?.jobs ?? {})) {
    const steps = job.steps ?? [];
    const pins = steps.find((s) => s.id === "pins");
    const reads = pins && String(pins.run).includes(".tool-versions") && String(pins.run).includes('"just"');
    const installs = steps.some((s) => s.with?.tool === PIN_STEP);
    if (!reads || !installs) errors.push(`ci.yml:${id} must install just at the .tool-versions pin (a 'pins' step + tool: ${PIN_STEP}).`);
  }

  // Packaging lockstep between the install-path job and the release build.
  const releaseBuild = workflows["release.yml"]?.jobs?.build?.steps ?? [];
  const installJob = ci?.jobs?.install?.steps ?? [];
  for (const name of ["Package (unix)", "Package (windows)"]) {
    const a = releaseBuild.find((s) => s.name === name);
    const b = installJob.find((s) => s.name === name);
    if (!a || !b) errors.push(`'${name}' must exist in both release.yml:build and ci.yml:install.`);
    else if (a.run !== b.run || a.shell !== b.shell || a.if !== b.if) {
      errors.push(`'${name}' differs between release.yml:build and ci.yml:install; keep them identical.`);
    }
  }
  // The pinned toolchain carries exactly the targets a release builds.
  let toolchainTargets = [];
  try {
    toolchainTargets = Bun.TOML.parse(readFileSync(join(root, "rust-toolchain.toml"), "utf8")).toolchain?.targets ?? [];
  } catch (err) {
    errors.push(`rust-toolchain.toml is not readable TOML: ${err.message}`);
  }
  const releaseTargets = (workflows["release.yml"]?.jobs?.build?.strategy?.matrix?.include ?? []).map((i) => i.target);
  if (!releaseTargets.length || !releaseTargets.every((t) => typeof t === "string")) {
    errors.push("release.yml's build job must list a string `target` in every matrix include entry.");
  }
  if (!Array.isArray(toolchainTargets) || !toolchainTargets.every((t) => typeof t === "string")) {
    errors.push("rust-toolchain.toml's [toolchain] targets must be a list of strings.");
    toolchainTargets = [];
  }
  const sorted = (xs) => [...xs].sort().join(", ");
  if (sorted(toolchainTargets) !== sorted(releaseTargets)) {
    errors.push(
      `rust-toolchain.toml targets [${sorted(toolchainTargets)}] differ from release.yml's build matrix [${sorted(releaseTargets)}]; keep them equal.`,
    );
  }
  return errors;
}

/** `[--root <existing dir>]`, nothing else; the default is the repository. */
function parseRoot(argv, fallback) {
  if (argv.length === 0) return fallback;
  if (argv.length === 2 && argv[0] === "--root" && existsSync(argv[1]) && statSync(argv[1]).isDirectory()) {
    return resolve(argv[1]);
  }
  console.error(`usage: bun ${process.argv[1]} [--root <existing directory>] (got: ${argv.join(" ") || "nothing"})`);
  process.exit(2);
}

if (import.meta.main) {
  const root = parseRoot(process.argv.slice(2), resolve(join(import.meta.dir, "..")));
  const errors = checkContract(root);
  if (errors.length) {
    for (const e of errors) console.error(`workflow-contract: ${e}`);
    process.exit(1);
  }
}
