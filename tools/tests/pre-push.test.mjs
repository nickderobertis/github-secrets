// The pre-push hook, activated the way `just bootstrap` activates it
// (core.hooksPath .githooks), driven by real `git push`es to a bare remote. A
// stand-in `llmlint` on PATH decides whether `llmlint validate` passes, and a
// PATH without one proves the skip.
import { afterEach, expect, test } from "bun:test";
import { chmodSync, cpSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { delimiter, dirname, join } from "node:path";
import { REPO, gitRepo, ok, run, scratch } from "./helpers.mjs";

const isWindows = process.platform === "win32";
let cleanups = [];
afterEach(() => {
  for (const c of cleanups) c();
  cleanups = [];
});

/** A clone-like repo with the hook activated, a bare origin, and a fake-llmlint dir. */
function setup(llmlintExit) {
  const s = scratch();
  cleanups.push(s.cleanup);
  const origin = join(s.dir, "origin.git");
  ok("git", ["init", "-q", "--bare", "-b", "master", origin]);
  const repo = join(s.dir, "work");
  ok("git", ["init", "-q", "-b", "master", repo]);
  const { git, commit } = gitRepo(repo);
  mkdirSync(join(repo, ".githooks"));
  cpSync(join(REPO, ".githooks/pre-push"), join(repo, ".githooks/pre-push"));
  git("config", "core.hooksPath", ".githooks");
  git("remote", "add", "origin", origin);
  commit("initial");

  const fakeBin = join(s.dir, "fake-bin");
  mkdirSync(fakeBin);
  const calls = join(s.dir, "llmlint-calls");
  if (llmlintExit !== null) {
    const fake = join(fakeBin, "llmlint");
    writeFileSync(
      fake,
      `#!/bin/sh\necho "$*" >> "${calls}"\n[ ${llmlintExit} -eq 0 ] || echo "validate: unknown rule in ignore directive" >&2\nexit ${llmlintExit}\n`,
    );
    chmodSync(fake, 0o755);
  }
  // Only the system tool dirs plus the fake: no real llmlint can resolve.
  const toolDirs = ["git", "bash", "sh"].map((t) => dirname(ok("bash", ["-c", `command -v ${t}`])));
  const PATH = [fakeBin, ...new Set([...toolDirs, "/usr/bin", "/bin"])].join(delimiter);
  const push = () => run("git", ["push", "origin", "master"], { cwd: repo, env: { ...process.env, PATH } });
  const remoteHead = () => run("git", ["rev-parse", "--verify", "--quiet", "master"], { cwd: origin }).stdout.trim();
  const llmlintOnPath = run("bash", ["-c", "command -v llmlint"], { env: { ...process.env, PATH } }).stdout.trim();
  return { repo, git, commit, push, remoteHead, calls, llmlintOnPath, fakeBin };
}

test.skipIf(isWindows)("a passing llmlint validate lets the push through", () => {
  const t = setup(0);
  const r = t.push();
  expect(r.code).toBe(0);
  expect(t.remoteHead()).toBe(t.git("rev-parse", "HEAD"));
  expect(readFileSync(t.calls, "utf8").trim()).toBe("validate");

  // Once origin/master exists, validate is scoped to the branch's changes.
  t.git("fetch", "-q", "origin");
  t.commit("second");
  expect(t.push().code).toBe(0);
  expect(readFileSync(t.calls, "utf8").trim().split("\n")[1]).toBe("validate --diff-base origin/master");
});

test.skipIf(isWindows)("a failing llmlint validate blocks the push", () => {
  const t = setup(1);
  const r = t.push();
  expect(r.code).not.toBe(0);
  expect(r.stderr).toContain("unknown rule in ignore directive");
  expect(r.stderr).toContain("pre-push: 'llmlint validate' failed");
  expect(t.remoteHead()).toBe("");
});

test.skipIf(isWindows)("without llmlint on PATH the hook skips with its message", () => {
  const t = setup(null);
  expect(t.llmlintOnPath).toBe("");
  const r = t.push();
  expect(r.code).toBe(0);
  expect(r.stderr).toContain("pre-push: llmlint is not installed; skipping 'llmlint validate'");
  expect(t.remoteHead()).toBe(t.git("rev-parse", "HEAD"));
});
