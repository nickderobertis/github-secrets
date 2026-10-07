// Decide which gate tier a CI run owes, and the explicit base it keys off — the
// placement AGENTS.md "Commits, releases, and merging" records: the release
// PR gets the full sweep, everything else the affected tier.
//
// Reads GITHUB_EVENT_NAME and the payload at GITHUB_EVENT_PATH and writes
// `tier=<affected|all>` and `base=<sha>` (empty for `all`) to GITHUB_OUTPUT,
// saying which in one line on stderr; outside Actions (no GITHUB_OUTPUT) the two
// lines go to stdout instead.
//
// Usage: bun scripts/ci-gate-tier.mjs
// Exit status: 0 with a decision; 1 when no decision can be made (the message
// names the cause and the next action).
import { execFileSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";
import { join } from "node:path";

// release-please names its release-PR branch `release-please--branches--<branch
// it releases from>` (plus a component suffix), so the prefix is derived from the
// branch release.yml runs on rather than restated here.
function releaseBranchPrefix() {
  let release;
  try {
    release = Bun.YAML.parse(readFileSync(join(import.meta.dir, "../.github/workflows/release.yml"), "utf8"));
  } catch (err) {
    throw new RoutingError(
      `cannot read .github/workflows/release.yml: ${err.message}`,
      "check out the full repository (the release-PR branch name is derived from release.yml).",
    );
  }
  const branches = release?.on?.push?.branches;
  if (!Array.isArray(branches) || branches.length !== 1 || typeof branches[0] !== "string") {
    throw new RoutingError(
      "release.yml must run on exactly one push branch to derive the release-PR branch from",
      "restore `on: push: branches: [<default branch>]` in .github/workflows/release.yml.",
    );
  }
  return `release-please--branches--${branches[0]}`;
}
const ZERO_SHA = /^0+$/;

/** A routing failure carrying the concrete next action for the CI log. */
class RoutingError extends Error {
  constructor(message, fix) {
    super(message);
    this.fix = fix;
  }
}

function git(args, cwd) {
  try {
    return execFileSync("git", args, { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
  } catch (err) {
    throw new RoutingError(
      `git ${args.join(" ")} failed: ${String(err.stderr || err.message).trim()}`,
      "check out with fetch-depth: 0 so the base branch and its history are present.",
    );
  }
}

function resolves(rev, cwd) {
  try {
    git(["rev-parse", "--verify", "--quiet", `${rev}^{commit}`], cwd);
    return true;
  } catch {
    return false;
  }
}

/** The routing decision for one event, from the payload, git history in `cwd`, and release.yml. */
export function decide(eventName, payload, cwd = process.cwd()) {
  if (eventName === "pull_request") {
    const pr = payload.pull_request ?? {};
    const sameRepo = pr.head?.repo?.full_name && pr.head.repo.full_name === pr.base?.repo?.full_name;
    if (sameRepo && typeof pr.head?.ref === "string" && pr.head.ref.startsWith(releaseBranchPrefix())) {
      return { tier: "all", base: "", why: `release PR (${pr.head.ref}): full sweep at release-prep` };
    }
    const baseRef = pr.base?.ref;
    if (typeof baseRef !== "string" || !/^[A-Za-z0-9._/-]+$/.test(baseRef) || baseRef.includes("..")) {
      throw new RoutingError(
        `pull_request payload has no usable base ref (${JSON.stringify(baseRef)})`,
        "target a branch whose name is letters, digits and . _ / - only; nothing was run.",
      );
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

function readPayload(path) {
  if (!path) return {};
  try {
    const payload = JSON.parse(readFileSync(path, "utf8"));
    if (payload === null || typeof payload !== "object" || Array.isArray(payload)) throw new Error("not a JSON object");
    return payload;
  } catch (err) {
    throw new RoutingError(
      `the event payload at GITHUB_EVENT_PATH (${path}) is unreadable: ${err.message}`,
      "run this inside a GitHub Actions job (it provides the payload), or point GITHUB_EVENT_PATH at a JSON event.",
    );
  }
}

if (import.meta.main) {
  let decision;
  try {
    decision = decide(process.env.GITHUB_EVENT_NAME ?? "", readPayload(process.env.GITHUB_EVENT_PATH));
  } catch (err) {
    console.error(`ci-gate-tier: ${err.message}`);
    if (err.fix) console.error(`ci-gate-tier: next: ${err.fix}`);
    process.exit(1);
  }
  const lines = `tier=${decision.tier}\nbase=${decision.base}\n`;
  if (!process.env.GITHUB_OUTPUT) {
    process.stdout.write(lines);
  } else {
    try {
      appendFileSync(process.env.GITHUB_OUTPUT, lines);
    } catch (err) {
      console.error(`ci-gate-tier: could not append to GITHUB_OUTPUT (${process.env.GITHUB_OUTPUT}): ${err.message}`);
      console.error("ci-gate-tier: next: run this inside a GitHub Actions step (it provides a writable GITHUB_OUTPUT), or unset it.");
      process.exit(1);
    }
    console.error(`ci-gate-tier: ${decision.tier} — ${decision.why}`);
  }
}
