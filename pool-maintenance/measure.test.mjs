// pool-maintenance/measure.sh end to end on a scratch Cargo workspace (two
// members, no dependencies) carrying a copy of the real justfile: real cargo,
// cargo-llvm-cov, nextest and cargo-prune-superseded build and prune it. The
// cases show that the untimed pass clears an older backlog, that the timed pass
// removes exactly the alternative build's entries and keeps every ordinary one,
// that the figure is the pruner's own report of that pass with the backlog pass
// only in its detail, and that nothing is built when there is nowhere to report.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

// Self-contained, like scripts/coverage's test: importing the scripts project's
// helpers would make this project depend on that one.
const REPO = resolve(import.meta.dir, "..");
const run = (cmd, args, opts = {}) => {
  const r = spawnSync(cmd, args, { encoding: "utf8", ...opts });
  if (r.error) throw r.error;
  return { code: r.status, stdout: r.stdout ?? "", stderr: r.stderr ?? "" };
};
const ok = (cmd, args, opts = {}) => {
  const r = run(cmd, args, opts);
  if (r.code !== 0) throw new Error(`${cmd} ${args.join(" ")} exited ${r.code}: ${r.stderr}`);
  return r.stdout.trim();
};

// measure.sh is Linux-only (GNU find/touch), like the hosts it measures for.
const linux = process.platform === "linux";
let dir, ws, env;

const write = (path, text) => {
  mkdirSync(join(ws, path, ".."), { recursive: true });
  writeFileSync(join(ws, path), text);
};

beforeAll(() => {
  dir = mkdtempSync(join(tmpdir(), "ghs-pool-maintenance-"));
  ws = join(dir, "ws");
  write("Cargo.toml", `[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n[workspace]\nmembers = ["tests/e2e"]\n`);
  write("src/lib.rs", `pub fn greet() -> &'static str {\n    "hi"\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn greets() {\n        assert_eq!(super::greet(), "hi");\n    }\n}\n`);
  write("tests/e2e/Cargo.toml", `[package]\nname = "app-e2e"\nversion = "0.1.0"\nedition = "2021"\n`);
  write("tests/e2e/src/lib.rs", "");
  write("tests/e2e/tests/run.rs", `#[test]\nfn runs() {}\n`);
  mkdirSync(join(ws, "pool-maintenance"));
  copyFileSync(join(REPO, "justfile"), join(ws, "justfile"));
  copyFileSync(join(REPO, "pool-maintenance/measure.sh"), join(ws, "pool-maintenance/measure.sh"));
  env = { ...process.env };
  for (const key of ["CARGO_TARGET_DIR", "ONEBUDGETSPEC_RESULT"]) delete env[key];
  if (linux) ok("cargo", ["generate-lockfile", "--offline"], { cwd: ws, env });
});
afterAll(() => rmSync(dir, { recursive: true, force: true }));

const measure = (extra = {}) => run("bash", ["pool-maintenance/measure.sh"], { cwd: ws, env: { ...env, ...extra } });

// Every build entry under target, as measure.sh lists them.
function entries() {
  const found = new Set();
  const walk = (d) => {
    for (const e of readdirSync(d, { withFileTypes: true })) {
      if (!e.isDirectory()) continue;
      const p = join(d, e.name);
      if (e.name === ".fingerprint") {
        for (const sub of ["deps", ".fingerprint", "build", "incremental"]) {
          const sd = join(d, sub);
          if (existsSync(sd)) for (const n of readdirSync(sd)) found.add(join(sd, n).slice(ws.length + 1));
        }
      } else if (!["deps", "build", "incremental"].includes(e.name)) walk(p);
    }
  };
  walk(join(ws, "target"));
  return found;
}
const lines = (file) => readFileSync(join(ws, "target/pool-maintenance-measure", file), "utf8").split("\n").filter(Boolean);

test.skipIf(process.platform === "win32")("without ONEBUDGETSPEC_RESULT it refuses before building anything", () => {
  const r = measure();
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("ONEBUDGETSPEC_RESULT is not set");
  expect(existsSync(join(ws, "target"))).toBe(false);
});

test.skipIf(!linux)("the timed pass removes exactly one run's superseded builds and reports itself", () => {
  // A backlog: the gate build under a third configuration, a week old.
  const llvmCov = (...prefix) => {
    for (const crate of ["app", "app-e2e"]) ok("env", [...prefix, "cargo", "llvm-cov", "--no-report", "nextest", "-p", crate, "--locked", "-E", "none()", "--no-tests=pass"], { cwd: ws, env });
  };
  llvmCov("CARGO_PROFILE_DEV_DEBUG=line-tables-only");
  const backlog = entries();
  const week = Date.now() / 1000 - 7 * 86400;
  const age = (p) => {
    const st = statSync(p);
    if (st.isDirectory()) for (const n of readdirSync(p)) age(join(p, n));
    utimesSync(p, week, week);
  };
  for (const e of backlog) age(join(ws, e));
  // The ordinary gate build, before any measurement.
  llvmCov();
  const ordinary = new Set([...entries()].filter((e) => !backlog.has(e)));
  expect(ordinary.size).toBeGreaterThan(0);

  const result = join(dir, "result.json");
  const r = measure({ ONEBUDGETSPEC_RESULT: result });
  expect(r.stderr).toMatch(/measure: alternative build created (\d+) entries; the timed pass removed \1 of them; ordinary entries before\/after: (\d+)\/\2 /);
  expect(r.code).toBe(0);

  const after = entries();
  for (const e of backlog) expect(after.has(e)).toBe(false); // the untimed pass cleared it
  for (const e of ordinary) expect(after.has(e)).toBe(true); // every ordinary entry survives
  const alternative = lines("alternative.txt");
  expect(alternative.length).toBeGreaterThan(0);
  for (const e of alternative) expect(ordinary.has(e) || backlog.has(e)).toBe(false);
  expect(lines("removed.txt")).toEqual(alternative); // the timed pass removed the alternative build, all of it

  const report = JSON.parse(readFileSync(result, "utf8"));
  expect(report.value).toBeGreaterThan(0);
  const removed = Number(report.detail.match(/builds removed: (\d+)/)?.[1]);
  expect(removed).toBeGreaterThan(0);
  expect(report.detail).toMatch(/entries walked: \d+; profile directories: \d+; builds removed: \d+; bytes freed: \d+; seconds walking: [\d.]+; seconds removing: [\d.]+; untimed backlog pass \(not in the figure\): [\d.]+ s$/);
  expect(r.stdout).toContain(`cargo-prune-superseded: target: removed ${removed} superseded build(s)`);
});
