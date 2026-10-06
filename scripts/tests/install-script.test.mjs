// scripts/install.sh's local-archive mode (--from-dir), which CI's install-path
// job uses: the same target detection, asset names, checksum verification and
// install as a download, with the archive read from disk. Archives here are
// packaged the way release.yml packages them (tar.gz under a leading
// gh-secrets-<tag>-<target>/ directory, `.sha256` appended to the archive name).
import { afterEach, expect, test } from "bun:test";
import { chmodSync, existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { REPO, ok, run, scratch } from "./helpers.mjs";

const isWindows = process.platform === "win32";
const TAG = "v9.9.9";
let cleanups = [];
afterEach(() => {
  for (const c of cleanups) c();
  cleanups = [];
});

function hostTarget() {
  const arch = { x64: "x86_64", arm64: "aarch64" }[process.arch];
  return process.platform === "darwin" ? `${arch}-apple-darwin` : `${arch}-unknown-linux-gnu`;
}

/** A release-shaped archive + checksum for this host in a fresh dir. */
function release({ corruptSum = false } = {}) {
  const s = scratch();
  cleanups.push(s.cleanup);
  const dist = `gh-secrets-${TAG}-${hostTarget()}`;
  mkdirSync(join(s.dir, dist));
  const bin = join(s.dir, dist, "gh-secrets");
  writeFileSync(bin, `#!/bin/sh\necho "gh-secrets ${TAG.slice(1)}"\n`);
  chmodSync(bin, 0o755);
  ok("tar", ["czf", `${dist}.tar.gz`, dist], { cwd: s.dir });
  const sum = ok("bash", ["-c", `(sha256sum "${dist}.tar.gz" 2>/dev/null || shasum -a 256 "${dist}.tar.gz")`], { cwd: s.dir });
  writeFileSync(join(s.dir, `${dist}.tar.gz.sha256`), corruptSum ? `${"0".repeat(64)}  ${dist}.tar.gz\n` : `${sum}\n`);
  return { dir: s.dir, dist, to: join(s.dir, "installed") };
}

const install = (args) => run("sh", [join(REPO, "scripts/install.sh"), ...args]);

test.skipIf(isWindows)("installs the local release archive and the binary runs", () => {
  const r = release();
  const res = install(["--version", TAG, "--from-dir", r.dir, "--to", r.to]);
  expect(res.code).toBe(0);
  expect(res.stderr.trim()).toBe(`installed gh-secrets ${TAG} to ${join(r.to, "gh-secrets")}\n\nNOTE: ${r.to} is not on your PATH. Add it to your shell profile:\n  export PATH="${r.to}:$PATH"`);
  expect(ok(join(r.to, "gh-secrets"), [])).toBe("gh-secrets 9.9.9");
});

test.skipIf(isWindows)("a missing archive fails and installs nothing", () => {
  const r = release();
  const res = install(["--version", "v0.0.1", "--from-dir", r.dir, "--to", r.to]);
  expect(res.code).not.toBe(0);
  expect(res.stderr).toContain(`gh-secrets-v0.0.1-${hostTarget()}.tar.gz not found in ${r.dir}`);
  expect(existsSync(join(r.to, "gh-secrets"))).toBe(false);
});

test.skipIf(isWindows)("a checksum mismatch refuses to install", () => {
  const r = release({ corruptSum: true });
  const res = install(["--version", TAG, "--from-dir", r.dir, "--to", r.to]);
  expect(res.code).not.toBe(0);
  expect(res.stderr).toContain("checksum mismatch");
  expect(existsSync(join(r.to, "gh-secrets"))).toBe(false);
});

test.skipIf(isWindows)("--from-dir without --version is refused", () => {
  const r = release();
  const res = install(["--from-dir", r.dir, "--to", r.to]);
  expect(res.code).not.toBe(0);
  expect(res.stderr).toContain("--from-dir needs --version");
});

test.skipIf(isWindows)("GH_SECRETS_ARCHIVE_DIR and --from-dir=<dir> are the same local mode", () => {
  const r = release();
  const viaEnv = run("sh", [join(REPO, "scripts/install.sh"), "--version", TAG, "--to", r.to], {
    env: { ...process.env, GH_SECRETS_ARCHIVE_DIR: r.dir },
  });
  expect(viaEnv.code).toBe(0);
  expect(ok(join(r.to, "gh-secrets"), [])).toBe("gh-secrets 9.9.9");

  const to2 = join(r.dir, "installed-2");
  expect(install([`--version=${TAG}`, `--from-dir=${r.dir}`, `--to=${to2}`]).code).toBe(0);
  expect(existsSync(join(to2, "gh-secrets"))).toBe(true);
});

test.skipIf(isWindows)("a --from-dir that does not exist is refused", () => {
  const r = release();
  const res = install(["--version", TAG, "--from-dir", join(r.dir, "nope"), "--to", r.to]);
  expect(res.code).not.toBe(0);
  expect(res.stderr).toContain("--from-dir: no such directory");
});

test.skipIf(isWindows)("an empty --from-dir= is refused rather than silently downloading", () => {
  const r = release();
  const res = install(["--version", TAG, "--from-dir=", "--to", r.to]);
  expect(res.code).not.toBe(0);
  expect(res.stderr).toContain("--from-dir needs a value");
});

test.skipIf(isWindows || process.getuid?.() === 0)("an unreadable local archive fails with what to check", () => {
  const r = release();
  chmodSync(join(r.dir, `${r.dist}.tar.gz`), 0o000);
  const res = install(["--version", TAG, "--from-dir", r.dir, "--to", r.to]);
  chmodSync(join(r.dir, `${r.dist}.tar.gz`), 0o644);
  expect(res.code).not.toBe(0);
  expect(res.stderr).toContain("check that it is readable");
  expect(existsSync(join(r.to, "gh-secrets"))).toBe(false);
});

test.skipIf(isWindows)("an archive without its .sha256 is not installed", () => {
  const r = release();
  rmSync(join(r.dir, `${r.dist}.tar.gz.sha256`));
  const res = install(["--version", TAG, "--from-dir", r.dir, "--to", r.to]);
  expect(res.code).not.toBe(0);
  expect(res.stderr).toContain(`${r.dist}.tar.gz.sha256 not found in ${r.dir}`);
  expect(existsSync(join(r.to, "gh-secrets"))).toBe(false);
});

test.skipIf(isWindows)("a version that is not a tag is refused before any path is built", () => {
  const r = release();
  const res = install(["--version", "../../etc", "--from-dir", r.dir, "--to", r.to]);
  expect(res.code).not.toBe(0);
  expect(res.stderr).toContain("invalid --version '../../etc'");
});
