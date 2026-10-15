// The declared pool maintenance, driven through the real justfile in scratch
// directories: onevcs.toml's command names a recipe the justfile declares,
// `pool-maintenance` runs the real cargo-prune-superseded over a target dir with
// real mtimes, and `install-prune-superseded` installs the pinned release only
// when that release is not already the one on PATH (cargo is a recorder there).
import { afterAll, beforeAll, expect, test } from "bun:test";
import { chmodSync, copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { delimiter, join } from "node:path";
import { REPO, ok, run, scratch } from "../../scripts/tests/helpers.mjs";

// The stubs are bash scripts, so these run on the Linux and macOS legs.
const isWindows = process.platform === "win32";
const DAY = 86400;
let s, work;

beforeAll(() => {
  if (isWindows) return;
  s = scratch();
  work = join(s.dir, "work");
  mkdirSync(work);
  copyFileSync(join(REPO, "justfile"), join(work, "justfile"));
});
afterAll(() => s?.cleanup());

const pinned = () => readFileSync(join(REPO, "justfile"), "utf8").match(/^prune_superseded_version := "([^"]+)"$/m)?.[1];

test.skipIf(isWindows)("onevcs.toml's command runs a recipe the justfile declares", () => {
  const declared = Bun.TOML.parse(readFileSync(join(REPO, "onevcs.toml"), "utf8"));
  expect(declared.schema_version).toBe(1);
  const [tool, recipe, ...rest] = declared.pool.maintain.command;
  expect([tool, rest]).toEqual(["just", []]);
  expect(ok("just", ["--show", recipe], { cwd: work })).toContain("cargo-prune-superseded --keep-within 2d target");
});

// A build entry of `unit` with metadata `hash` in target/debug, modified `age` days ago.
function build(unit, hash, age) {
  const debug = join(work, "target", "debug");
  const paths = [join(debug, ".fingerprint", `${unit}-${hash}`), join(debug, "deps", `lib${unit}-${hash}.rlib`)];
  mkdirSync(paths[0], { recursive: true });
  mkdirSync(join(debug, "deps"), { recursive: true });
  writeFileSync(join(paths[0], "lib-" + unit), hash);
  writeFileSync(paths[1], "x".repeat(4096));
  const when = Date.now() / 1000 - age * DAY;
  for (const p of [join(paths[0], "lib-" + unit), ...paths]) utimesSync(p, when, when);
  return paths;
}

test.skipIf(isWindows || run("cargo-prune-superseded", ["--version"]).code !== 0)(
  "pool-maintenance removes superseded builds older than 2 days and keeps the newest and the recent",
  () => {
    rmSync(join(work, "target"), { recursive: true, force: true });
    const stale = build("app", "1111111111111111", 5);
    const recent = build("app", "2222222222222222", 1);
    const newest = build("app", "3333333333333333", 0);
    const lone = build("other", "4444444444444444", 9);
    const r = run("just", ["pool-maintenance"], { cwd: work });
    expect(r.code).toBe(0);
    expect(r.stdout).toContain("cargo-prune-superseded: target: removed 1 superseded build(s) across 1 profile(s)");
    for (const p of stale) expect(existsSync(p)).toBe(false);
    for (const p of [...recent, ...newest, ...lone]) expect(existsSync(p)).toBe(true);
  },
);

// A PATH whose stub `cargo` records its arguments and, on `cargo install`, puts a
// `cargo-prune-superseded` of the requested version on that PATH; `installed`
// seeds the pruner already there (null: none, shadowing any real one on PATH).
// Every other tool a recipe calls is a no-op stub.
function stubPath(installed) {
  const bin = join(s.dir, "bin");
  rmSync(bin, { recursive: true, force: true });
  mkdirSync(bin);
  const record = join(s.dir, "cargo-args");
  rmSync(record, { force: true });
  const pruner = (v) => `#!/usr/bin/env bash\necho "cargo-prune-superseded ${v}"\n`;
  writeFileSync(
    join(bin, "cargo"),
    `#!/usr/bin/env bash\nprintf '%s\\n' "$*" >> "${record}"\n` +
      `if [ "$1" = install ]; then printf '#!/usr/bin/env bash\\necho "cargo-prune-superseded %s"\\n' "\${@: -1}" > "${join(bin, "cargo-prune-superseded")}"; chmod +x "${join(bin, "cargo-prune-superseded")}"; fi\n`,
  );
  writeFileSync(join(bin, "cargo-prune-superseded"), installed === null ? "#!/usr/bin/env bash\nexit 127\n" : pruner(installed));
  for (const f of ["rustup", "npm"]) writeFileSync(join(bin, f), "#!/usr/bin/env bash\n");
  for (const f of readdirSync(bin)) chmodSync(join(bin, f), 0o755);
  const env = { ...process.env, CI: "1", PATH: `${bin}${delimiter}${process.env.PATH}` };
  const installs = () =>
    (existsSync(record) ? readFileSync(record, "utf8") : "").split("\n").filter((l) => l.startsWith("install --locked cargo-prune-superseded"));
  return { env, installs };
}

test.skipIf(isWindows)("install-prune-superseded installs the pinned release unless it is the one on PATH", () => {
  const version = pinned();
  expect(version).toMatch(/^\d+\.\d+\.\d+$/);
  const install = `install --locked cargo-prune-superseded --version ${version}`;

  const missing = stubPath(null);
  expect(run("just", ["install-prune-superseded"], { cwd: work, env: missing.env }).code).toBe(0);
  expect(missing.installs()).toEqual([install]);

  const stale = stubPath("0.0.1");
  expect(run("just", ["install-prune-superseded"], { cwd: work, env: stale.env }).code).toBe(0);
  expect(stale.installs()).toEqual([install]);

  const current = stubPath(version);
  expect(run("just", ["install-prune-superseded"], { cwd: work, env: current.env }).code).toBe(0);
  expect(current.installs()).toEqual([]);
});

test.skipIf(isWindows)("bootstrap installs the pinned pruner on a fresh host, and a second bootstrap does not reinstall it", () => {
  const version = pinned();
  // The scripts bootstrap runs, as no-ops: this journey is about the pruner it installs.
  mkdirSync(join(work, "scripts"), { recursive: true });
  for (const f of ["install-nextest.sh", "bun.sh", "nx"]) writeFileSync(join(work, "scripts", f), "");
  const host = stubPath(null);

  const first = run("just", ["bootstrap"], { cwd: work, env: host.env });
  expect(first.code, first.stderr).toBe(0);
  expect(host.installs()).toEqual([`install --locked cargo-prune-superseded --version ${version}`]);
  expect(ok("cargo-prune-superseded", ["--version"], { env: host.env })).toBe(`cargo-prune-superseded ${version}`);

  const second = run("just", ["bootstrap"], { cwd: work, env: host.env });
  expect(second.code, second.stderr).toBe(0);
  expect(host.installs()).toHaveLength(1);
});

// The nx arguments a recipe's `all` sweep passes, read from the justfile.
function sweepArgs(recipe) {
  const shown = ok("just", ["--show", recipe], { cwd: REPO });
  const sweep = shown.match(/all\) bash scripts\/nx (.+?) ;;/);
  if (!sweep) throw new Error(`just --show ${recipe} has no \`all) bash scripts/nx ... ;;\` arm; update sweepArgs to the recipe's shape:\n${shown}`);
  return sweep[1];
}

// The tasks of the graph real Nx builds for `args`.
function tasks(args) {
  const graph = join(s.dir, "graph.json");
  ok("bash", ["scripts/nx", ...args.split(" "), `--graph=${graph}`], { cwd: REPO });
  return Object.keys(JSON.parse(readFileSync(graph, "utf8")).tasks.tasks);
}

test.skipIf(isWindows)("check and budgets never select the pool-maintenance budget, which the release-PR recipe runs", () => {
  const check = tasks(sweepArgs("check"));
  // The project's cheap targets ride the gate; its budget never does.
  expect(check).toEqual(expect.arrayContaining(["pool-maintenance:lint", "pool-maintenance:test"]));
  expect(check.filter((t) => t.startsWith("pool-maintenance:budgets"))).toEqual([]);
  expect(tasks(sweepArgs("budgets")).filter((t) => t.startsWith("pool-maintenance:"))).toEqual([]);
  // The project carries the standard budgets-host target: only naming it selects the budget.
  expect(tasks(`${sweepArgs("budgets")} budgets-host`)).toContain("pool-maintenance:budgets-host");
  expect(ok("just", ["--show", "pool-maintenance-budget"], { cwd: REPO })).toContain("scripts/nx run pool-maintenance:budgets-host");
}, 60_000);
