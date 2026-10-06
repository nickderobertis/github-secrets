// The gate recipes' tier and base selection, driven through the real justfile
// and scripts/nx-base.sh in a scratch repository with an origin. Only the
// orchestrator underneath (`scripts/nx`) is replaced by a recorder, so each case
// shows exactly what would have been handed to Nx — or that nothing was.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { chmodSync, copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { REPO, gitRepo, ok, run, scratch } from "../../scripts/tests/helpers.mjs";

// The recorder is a bash stub with the scratch path spliced in, so these run on
// the Linux and macOS legs (the recipes and script are the same files there).
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
  copyFileSync(join(REPO, "justfile"), join(repo, "justfile"));
  copyFileSync(join(REPO, "scripts/nx-base.sh"), join(repo, "scripts/nx-base.sh"));
  record = join(s.dir, "nx-args");
  writeFileSync(join(repo, "scripts/nx"), `#!/usr/bin/env bash\nprintf '%s\\n' "$*" > "${record}"\n`);
  chmodSync(join(repo, "scripts/nx"), 0o755);
  git("remote", "add", "origin", origin);
  forkPoint = commit("base");
  git("push", "-q", "origin", "master");
  git("checkout", "-q", "-b", "feature");
  commit("feature work");
  git("fetch", "-q", "origin");
});
afterAll(() => s?.cleanup());

function just(args, nxBase) {
  rmSync(record, { force: true });
  const env = { ...process.env };
  delete env.NX_BASE;
  if (nxBase !== undefined) env.NX_BASE = nxBase;
  const r = run("just", args, { cwd: repo, env });
  return { ...r, nx: existsSync(record) ? readFileSync(record, "utf8").trim() : null };
}

const GATE = "-t format-check lint build test coverage";

test.skipIf(isWindows)("check without NX_BASE runs the affected tier from the merge base with origin/master", () => {
  const r = just(["check"]);
  expect(r.code).toBe(0);
  expect(r.nx).toBe(`affected --base=${forkPoint} ${GATE}`);
  expect(r.stderr).toContain("merge-base with origin/master");
});

test.skipIf(isWindows)("a valid NX_BASE (ref name or SHA) is the base", () => {
  expect(just(["check"], forkPoint).nx).toBe(`affected --base=${forkPoint} ${GATE}`);
  expect(just(["test"], "origin/master").nx).toBe("affected --base=origin/master -t test");
});

test.skipIf(isWindows)("an invalid NX_BASE is refused before anything runs, naming NX_BASE", () => {
  for (const bad of ["master;touch pwned", "$(id)", "a..b", "--output=x", "", "no-such-ref"]) {
    const r = just(["check"], bad);
    expect(r.code).not.toBe(0);
    expect(r.stderr).toContain("NX_BASE");
    expect(r.nx).toBeNull();
  }
  expect(existsSync(join(repo, "pwned"))).toBe(false);
});

test.skipIf(isWindows)("`all` is one run-many sweep over every project", () => {
  expect(just(["check", "all"]).nx).toBe(`run-many --all ${GATE}`);
  expect(just(["lint", "all"]).nx).toBe("run-many --all -t lint");
  expect(just(["format-check", "all"]).nx).toBe("run-many --all -t format-check");
});

test.skipIf(isWindows)("an unknown tier is refused and runs nothing", () => {
  const r = just(["check", "sometimes"]);
  expect(r.code).not.toBe(0);
  expect(r.stderr).toContain("unknown tier 'sometimes'");
  expect(r.nx).toBeNull();
});

test.skipIf(isWindows)("bench forwards a plain baseline and Criterion options, and refuses anything else", () => {
  expect(just(["bench", "pr", "--measurement-time", "3"]).nx).toBe("run gh-secrets-bench:bench --baseline=pr --criterion=--measurement-time 3");
  const r = just(["bench", "pr", "x;touch pwned"]);
  expect(r.code).not.toBe(0);
  expect(r.stderr).toContain("bench: argument 'x;touch pwned' is not a plain baseline name or Criterion option");
  expect(r.nx).toBeNull();
  expect(existsSync(join(repo, "pwned"))).toBe(false);
});
