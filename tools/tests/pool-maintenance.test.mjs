// The declared pool maintenance, driven through the real justfile in scratch
// directories: onevcs.toml's command names a recipe the justfile declares,
// `pool-maintenance` runs the real cargo-prune-superseded over a target dir with
// real mtimes, and `install-prune-superseded` installs the pinned release only
// when that release is not already the one on PATH (cargo is a recorder there).
import { afterAll, beforeAll, expect, test } from "bun:test";
import { chmodSync, copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, utimesSync, writeFileSync } from "node:fs";
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

// PATH with stub `cargo` (recording its arguments) and `cargo-prune-superseded` first.
function stubs(installed) {
  const bin = join(s.dir, "bin");
  rmSync(bin, { recursive: true, force: true });
  mkdirSync(bin);
  const record = join(s.dir, "cargo-args");
  rmSync(record, { force: true });
  writeFileSync(join(bin, "cargo"), `#!/usr/bin/env bash\nprintf '%s\\n' "$*" >> "${record}"\n`);
  writeFileSync(
    join(bin, "cargo-prune-superseded"),
    installed === null ? "#!/usr/bin/env bash\nexit 127\n" : `#!/usr/bin/env bash\necho "cargo-prune-superseded ${installed}"\n`,
  );
  for (const f of ["cargo", "cargo-prune-superseded"]) chmodSync(join(bin, f), 0o755);
  const r = run("just", ["install-prune-superseded"], { cwd: work, env: { ...process.env, PATH: `${bin}${delimiter}${process.env.PATH}` } });
  return { ...r, cargo: existsSync(record) ? readFileSync(record, "utf8").trim() : null };
}

test.skipIf(isWindows)("install-prune-superseded installs the pinned release unless it is the one on PATH", () => {
  const version = pinned();
  expect(version).toMatch(/^\d+\.\d+\.\d+$/);
  const install = `install --locked cargo-prune-superseded --version ${version}`;

  const missing = stubs(null);
  expect(missing.code).toBe(0);
  expect(missing.cargo).toBe(install);

  expect(stubs("0.0.1").cargo).toBe(install);

  const current = stubs(version);
  expect(current.code).toBe(0);
  expect(current.cargo).toBeNull();
});
