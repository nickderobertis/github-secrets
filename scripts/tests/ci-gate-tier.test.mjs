// CI's tier routing (scripts/ci-gate-tier.mjs), fed synthetic event payloads
// against a real scratch repository with an origin, exactly as the workflow
// step runs it: env in, `tier=`/`base=` lines out (stdout and GITHUB_OUTPUT).
import { afterAll, beforeAll, expect, test } from "bun:test";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { REPO, gitRepo, ok, run, scratch } from "./helpers.mjs";

const SCRIPT = join(REPO, "scripts/ci-gate-tier.mjs");
let s, repo, forkPoint, masterTip, featureTip;

beforeAll(() => {
  s = scratch();
  const origin = join(s.dir, "origin.git");
  ok("git", ["init", "-q", "--bare", "-b", "master", origin]);
  repo = join(s.dir, "work");
  ok("git", ["init", "-q", "-b", "master", repo]);
  const { git, commit } = gitRepo(repo);
  git("remote", "add", "origin", origin);
  commit("one");
  forkPoint = commit("two");
  git("push", "-q", "origin", "master");
  git("checkout", "-q", "-b", "feature");
  featureTip = commit("feature work");
  git("checkout", "-q", "master");
  masterTip = commit("master moves on");
  git("push", "-q", "origin", "master");
  git("fetch", "-q", "origin");
  git("checkout", "-q", "feature");
});
afterAll(() => s.cleanup());

function route(eventName, payload) {
  const event = join(s.dir, "event.json");
  const output = join(s.dir, `output-${Math.random()}`);
  writeFileSync(event, JSON.stringify(payload));
  writeFileSync(output, "");
  const r = run("bun", [SCRIPT], {
    cwd: repo,
    env: { ...process.env, GITHUB_EVENT_NAME: eventName, GITHUB_EVENT_PATH: event, GITHUB_OUTPUT: output },
  });
  const parsed = Object.fromEntries(
    readFileSync(output, "utf8").trim().split("\n").filter(Boolean).map((l) => l.split("=")),
  );
  return { ...r, ...parsed };
}

const pr = (headRef, headRepo = "nickderobertis/github-secrets") => ({
  pull_request: {
    head: { ref: headRef, repo: { full_name: headRepo } },
    base: { ref: "master", repo: { full_name: "nickderobertis/github-secrets" } },
  },
});

test("a release-please branch containing source changes keeps the affected tier", () => {
  const r = route("pull_request", pr("release-please--branches--master--components--gh-secrets"));
  expect(r.code).toBe(0);
  expect(r.tier).toBe("affected");
  expect(r.base).toBe(forkPoint);
  expect(r.stdout).toBe("");
  expect(r.stderr.trim().split("\n")).toHaveLength(1);
});

test("an ordinary pull request runs the affected tier from the merge base", () => {
  const r = route("pull_request", pr("feature"));
  expect(r.code).toBe(0);
  expect(r.tier).toBe("affected");
  expect(r.base).toBe(forkPoint);
});

test("a fork borrowing the release branch name does not get the sweep", () => {
  const r = route("pull_request", pr("release-please--branches--master", "someone/github-secrets"));
  expect(r.tier).toBe("affected");
  expect(r.base).toBe(forkPoint);
});

test("a push to master runs the affected tier from event.before", () => {
  const r = route("push", { before: forkPoint, ref: "refs/heads/master" });
  expect(r.tier).toBe("affected");
  expect(r.base).toBe(forkPoint);
});

test("a push without a usable before falls back to HEAD~1", () => {
  const r = route("push", { before: "0".repeat(40) });
  expect(r.tier).toBe("affected");
  expect(r.base).toBe(ok("git", ["rev-parse", "HEAD~1"], { cwd: repo }));
  expect(r.base).not.toBe(featureTip);
});

test("a manual dispatch runs the full sweep", () => {
  const r = route("workflow_dispatch", {});
  expect(r.tier).toBe("all");
});

test("a pull request whose base ref is not a plain ref name is refused", () => {
  const payload = pr("feature");
  payload.pull_request.base.ref = "master; curl evil";
  const r = route("pull_request", payload);
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("no usable base ref");
  expect(masterTip).toBeTruthy();
});

test("an unreadable event payload fails with the next action, not a stack trace", () => {
  const event = join(s.dir, "broken-event.json");
  writeFileSync(event, "{ not json");
  const r = run("bun", [SCRIPT], { cwd: repo, env: { ...process.env, GITHUB_EVENT_NAME: "push", GITHUB_EVENT_PATH: event } });
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("is unreadable");
  expect(r.stderr).toContain("ci-gate-tier: next:");
});

test("a base branch the clone lacks names fetch-depth as the fix", () => {
  const payload = pr("feature");
  payload.pull_request.base.ref = "no-such-branch";
  const r = route("pull_request", payload);
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("git merge-base origin/no-such-branch HEAD failed");
  expect(r.stderr).toContain("fetch-depth: 0");
});

test("a JSON array payload is not an event", () => {
  const event = join(s.dir, "array-event.json");
  writeFileSync(event, "[]");
  const r = run("bun", [SCRIPT], { cwd: repo, env: { ...process.env, GITHUB_EVENT_NAME: "push", GITHUB_EVENT_PATH: event } });
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("not a JSON object");
});

test("an unwritable GITHUB_OUTPUT fails with the next action", () => {
  const event = join(s.dir, "dispatch.json");
  writeFileSync(event, "{}");
  const r = run("bun", [SCRIPT], {
    cwd: repo,
    env: { ...process.env, GITHUB_EVENT_NAME: "workflow_dispatch", GITHUB_EVENT_PATH: event, GITHUB_OUTPUT: join(s.dir, "no-such-dir", "out") },
  });
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("could not append to GITHUB_OUTPUT");
});

test("outside Actions (no GITHUB_OUTPUT) the decision goes to stdout", () => {
  const event = join(s.dir, "dispatch2.json");
  writeFileSync(event, "{}");
  const env = { ...process.env, GITHUB_EVENT_NAME: "workflow_dispatch", GITHUB_EVENT_PATH: event };
  delete env.GITHUB_OUTPUT;
  const r = run("bun", [SCRIPT], { cwd: repo, env });
  expect(r.code).toBe(0);
  expect(r.stdout).toBe("tier=all\nbase=\n");
  expect(r.stderr).toBe("");
});


test("semantic release-only PR gets the full sweep through the production classifier", () => {
  const local = scratch();
  try {
    const { git, commit } = gitRepo(local.dir);
    writeFileSync(join(local.dir, "Cargo.toml"), '[package]\nname="gh-secrets"\nversion="1.0.0"\nedition="2021"\n');
    commit("initial");
    git("update-ref", "refs/remotes/origin/master", "HEAD");
    writeFileSync(join(local.dir, "Cargo.toml"), '[package]\nname="gh-secrets"\nversion="1.1.0"\nedition="2021"\n');
    commit("version");
    const event = join(local.dir, ".git/event.json");
    writeFileSync(event, JSON.stringify(pr("release-please--branches--master--components--gh-secrets")));
    const r = run("bun", [SCRIPT], { cwd: local.dir, env: { ...process.env, GITHUB_EVENT_NAME: "pull_request", GITHUB_EVENT_PATH: event, GITHUB_OUTPUT: "" } });
    expect(r.code).toBe(0);
    expect(r.stdout).toBe("tier=all\nbase=\n");
  } finally { local.cleanup(); }
});
