// Decide which gate tier a CI run owes, and the explicit base it keys off.
//
// The placement (AGENTS.md "Commits, releases, and merging"): this repo batches
// releases behind release-please's release PR, so
//   * the release PR (head branch release-please--branches--master…, opened in
//     this repository) runs the BROADER tier — one full `just check all` sweep,
//     at release-prep, over the exact tree that ships;
//   * every other pull request, and every push to master, runs the AFFECTED tier
//     against an explicitly derived base:
//       pull request -> git merge-base origin/<base branch> HEAD
//       push         -> the event's `before` commit (the previous master tip),
//                       or HEAD~1 when `before` is absent or unknown here;
//   * anything else (workflow_dispatch) runs the full sweep.
//
// Reads GITHUB_EVENT_NAME and the payload at GITHUB_EVENT_PATH, prints
// `tier=<affected|all>` and `base=<sha>` (empty for `all`), and appends the same
// lines to GITHUB_OUTPUT when that is set. The CI step then runs
// `NX_BASE=<base> just check <tier>`.
//
// Usage: bun scripts/ci-gate-tier.mjs
import { execFileSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";

export const RELEASE_BRANCH_PREFIX = "release-please--branches--master";
const ZERO_SHA = /^0+$/;

function git(args, cwd) {
  return execFileSync("git", args, { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
}

function resolves(rev, cwd) {
  try {
    git(["rev-parse", "--verify", "--quiet", `${rev}^{commit}`], cwd);
    return true;
  } catch {
    return false;
  }
}

/** The routing decision for one event. Pure apart from the git lookups in `cwd`. */
export function decide(eventName, payload, cwd = process.cwd()) {
  if (eventName === "pull_request") {
    const pr = payload.pull_request ?? {};
    const sameRepo = pr.head?.repo?.full_name && pr.head.repo.full_name === pr.base?.repo?.full_name;
    if (sameRepo && typeof pr.head?.ref === "string" && pr.head.ref.startsWith(RELEASE_BRANCH_PREFIX)) {
      return { tier: "all", base: "", why: `release PR (${pr.head.ref}): full sweep at release-prep` };
    }
    const baseRef = pr.base?.ref;
    if (typeof baseRef !== "string" || !/^[A-Za-z0-9._/-]+$/.test(baseRef)) {
      throw new Error(`pull_request payload has no usable base ref (${JSON.stringify(baseRef)})`);
    }
    const base = git(["merge-base", `origin/${baseRef}`, "HEAD"], cwd);
    return { tier: "affected", base, why: `pull request: merge base with origin/${baseRef}` };
  }
  if (eventName === "push") {
    const before = payload.before;
    if (typeof before === "string" && /^[0-9a-f]{40}$/.test(before) && !ZERO_SHA.test(before) && resolves(before, cwd)) {
      return { tier: "affected", base: before, why: "push: the previous tip (event.before)" };
    }
    return { tier: "affected", base: git(["rev-parse", "HEAD~1"], cwd), why: "push: HEAD~1 (event.before unavailable)" };
  }
  return { tier: "all", base: "", why: `${eventName || "unknown event"}: full sweep` };
}

if (import.meta.main) {
  const eventName = process.env.GITHUB_EVENT_NAME ?? "";
  const payload = process.env.GITHUB_EVENT_PATH
    ? JSON.parse(readFileSync(process.env.GITHUB_EVENT_PATH, "utf8"))
    : {};
  let decision;
  try {
    decision = decide(eventName, payload);
  } catch (err) {
    console.error(`ci-gate-tier: ${err.message}`);
    console.error("ci-gate-tier: check out with fetch-depth: 0 so the base branch and history are present.");
    process.exit(1);
  }
  console.error(`ci-gate-tier: ${decision.tier} — ${decision.why}`);
  const lines = `tier=${decision.tier}\nbase=${decision.base}\n`;
  process.stdout.write(lines);
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, lines);
}
