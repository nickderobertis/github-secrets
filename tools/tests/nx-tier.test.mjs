// The gate's tier/base selection (scripts/nx-tier.sh), the code every gate
// recipe goes through. Run for real in a scratch repository with an origin;
// only the orchestrator underneath (scripts/nx.sh) is replaced by a recorder,
// so each case shows exactly what would have been handed to Nx — or that
// nothing was.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { REPO, gitRepo, ok, run, scratch } from "./helpers.mjs";

// Bash-script tests: the scratch repo's Windows paths would be spliced into a
// bash stub, so they run on the Linux and macOS legs (the script is the same file).
const isWindows = process.platform === "win32";
let s, repo, forkPoint, record;

beforeAll(() => {
  if (isWindows) return;
  s = scratch();
  const origin = join(s.dir, "origin.git");
  ok("git", ["init", "-q", "--bare", "-b", "master", origin]);
  repo = join(s.dir, "work");
  ok("git", ["init", "-q", "-b", "master", repo]);
  const { git, commit } = gitRepo(repo);
  mkdirSync(join(repo, "scripts"));
  copyFileSync(join(REPO, "scripts/nx-tier.sh"), join(repo, "scripts/nx-tier.sh"));
  record = join(s.dir, "nx-args");
  writeFileSync(join(repo, "scripts/nx.sh"), `#!/usr/bin/env bash\nprintf '%s\\n' "$*" > "${record}"\n`);
  git("remote", "add", "origin", origin);
  forkPoint = commit("base");
  git("push", "-q", "origin", "master");
  git("checkout", "-q", "-b", "feature");
  commit("feature work");
  git("fetch", "-q", "origin");
});
afterAll(() => s?.cleanup());

function tier(args, nxBase) {
  rmSync(record, { force: true });
  const env = { ...process.env };
  delete env.NX_BASE;
  if (nxBase !== undefined) env.NX_BASE = nxBase;
  const r = run("bash", ["scripts/nx-tier.sh", ...args], { cwd: repo, env });
  return { ...r, nx: existsSync(record) ? readFileSync(record, "utf8").trim() : null };
}

test.skipIf(isWindows)("without NX_BASE the affected tier keys off the merge base with origin/master", () => {
  const r = tier(["affected", "format-check", "lint"]);
  expect(r.code).toBe(0);
  expect(r.nx).toBe(`affected --base=${forkPoint} -t format-check lint`);
  expect(r.stderr).toContain("merge-base with origin/master");
});

test.skipIf(isWindows)("a valid NX_BASE (ref name or SHA) is the base", () => {
  expect(tier(["affected", "test"], forkPoint).nx).toBe(`affected --base=${forkPoint} -t test`);
  expect(tier(["affected", "test"], "origin/master").nx).toBe("affected --base=origin/master -t test");
});

test.skipIf(isWindows)("an invalid NX_BASE is refused before anything runs, naming NX_BASE", () => {
  for (const bad of ["master;touch pwned", "$(id)", "a..b", "--output=x", "", "no-such-ref"]) {
    const r = tier(["affected", "lint"], bad);
    expect(r.code).toBe(1);
    expect(r.stderr).toContain("NX_BASE");
    expect(r.nx).toBeNull();
  }
  expect(existsSync(join(repo, "pwned"))).toBe(false);
});

test.skipIf(isWindows)("the all tier is a run-many sweep and ignores NX_BASE", () => {
  expect(tier(["all", "format-check", "lint"], "not valid;").nx).toBe("run-many --all -t format-check lint");
});

test.skipIf(isWindows)("an unknown tier is refused", () => {
  const r = tier(["sometimes", "lint"]);
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("unknown tier 'sometimes'");
  expect(r.nx).toBeNull();
});
