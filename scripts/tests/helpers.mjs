// Shared helpers for the tooling tests: real subprocesses in scratch directories.
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

export const REPO = resolve(import.meta.dir, "../..");

/** Run a command; return { code, stdout, stderr }. Never throws on non-zero. */
export function run(cmd, args, opts = {}) {
  const r = spawnSync(cmd, args, { encoding: "utf8", ...opts });
  if (r.error) throw r.error;
  return { code: r.status, stdout: r.stdout ?? "", stderr: r.stderr ?? "" };
}

/** Run and require success; return trimmed stdout. */
export function ok(cmd, args, opts = {}) {
  const r = run(cmd, args, opts);
  if (r.code !== 0) throw new Error(`${cmd} ${args.join(" ")} exited ${r.code}: ${r.stderr}`);
  return r.stdout.trim();
}

/** A scratch directory removed by the returned cleanup. */
export function scratch(prefix = "ghs-tools-") {
  const dir = mkdtempSync(join(tmpdir(), prefix));
  return { dir, cleanup: () => rmSync(dir, { recursive: true, force: true }) };
}

/** A git repo in `dir` with one identity, and a helper to commit everything. */
export function gitRepo(dir) {
  const git = (...args) => ok("git", args, { cwd: dir });
  git("init", "-q", "-b", "master");
  git("config", "user.email", "tests@example.invalid");
  git("config", "user.name", "tests");
  git("config", "commit.gpgsign", "false");
  const commit = (message) => {
    git("add", "-A");
    git("commit", "-q", "--allow-empty", "-m", message);
    return git("rev-parse", "HEAD");
  };
  return { git, commit };
}
