// scripts/bun.sh — the bun pin's resolver and installer — run for real in a
// scratch checkout with its own .tool-versions pin. The download path is driven
// offline: GH_SECRETS_BUN_DOWNLOAD_BASE points curl at a file:// "release" laid
// out exactly like bun's GitHub release (bun-v<ver>/<asset>.zip + SHASUMS256.txt).
import { afterEach, expect, test } from "bun:test";
import { chmodSync, copyFileSync, existsSync, mkdirSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { REPO, ok, run, scratch } from "./helpers.mjs";

const isWindows = process.platform === "win32";
const PIN = "9.9.9";
const ASSET = {
  "linux/x64": "bun-linux-x64",
  "linux/arm64": "bun-linux-aarch64",
  "darwin/arm64": "bun-darwin-aarch64",
  "darwin/x64": "bun-darwin-x64",
}[`${process.platform}/${process.arch}`];
let cleanups = [];
afterEach(() => {
  for (const c of cleanups) c();
  cleanups = [];
});

function fakeBun(path, version) {
  writeFileSync(path, `#!/bin/sh\n[ "$1" = "--version" ] && echo ${version}\n`);
  chmodSync(path, 0o755);
}

/** A checkout with scripts/bun.sh and a pin, plus PATH/cache/release dirs. */
function setup({ pin = PIN, pathBunVersion = null, systemDirs = ["/usr/bin", "/bin"], extraEnv = {} } = {}) {
  const s = scratch();
  cleanups.push(s.cleanup);
  const repo = join(s.dir, "repo");
  mkdirSync(join(repo, "scripts"), { recursive: true });
  copyFileSync(join(REPO, "scripts/bun.sh"), join(repo, "scripts/bun.sh"));
  writeFileSync(join(repo, ".tool-versions"), `just 1.51.0\nbun ${pin}\n`);
  const pathDir = join(s.dir, "path-bin");
  mkdirSync(pathDir);
  if (pathBunVersion) fakeBun(join(pathDir, "bun"), pathBunVersion);
  const tools = join(s.dir, "tools");
  const env = {
    ...process.env,
    PATH: [pathDir, ...systemDirs].join(":"),
    GH_SECRETS_TOOLS_DIR: tools,
    GH_SECRETS_BUN_DOWNLOAD_BASE: `file://${join(s.dir, "release")}`,
    ...extraEnv,
  };
  const bunSh = (mode) => run("bash", ["scripts/bun.sh", mode], { cwd: repo, env });
  return { dir: s.dir, repo, tools, pathDir, bunSh };
}

/**
 * Lay out a fake bun release. `corrupt` publishes a wrong checksum, `noSums`
 * none at all, `reports` the version the packed bun claims, `empty` packs no bun.
 */
function publishRelease(dir, { corrupt = false, noSums = false, reports = PIN, empty = false, garbage = false } = {}) {
  const rel = join(dir, "release", `bun-v${PIN}`);
  const stage = join(dir, "stage", ASSET);
  mkdirSync(rel, { recursive: true });
  mkdirSync(stage, { recursive: true });
  if (empty) writeFileSync(join(stage, "README"), "no bun here\n");
  else fakeBun(join(stage, "bun"), reports);
  ok("python3", ["-I", "-c", `import shutil; shutil.make_archive(${JSON.stringify(join(rel, ASSET))}, "zip", ${JSON.stringify(join(dir, "stage"))}, ${JSON.stringify(ASSET)})`]);
  if (garbage) writeFileSync(join(rel, `${ASSET}.zip`), "this is not a zip archive\n");
  const sum = ok("bash", ["-c", `(sha256sum "${ASSET}.zip" 2>/dev/null || shasum -a 256 "${ASSET}.zip") | awk '{print $1}'`], { cwd: rel });
  if (!noSums) writeFileSync(join(rel, "SHASUMS256.txt"), `${corrupt ? "0".repeat(64) : sum}  ${ASSET}.zip\n`);
}

test.skipIf(isWindows)("a bun on PATH reporting the pin is used as-is", () => {
  const t = setup({ pathBunVersion: PIN });
  expect(t.bunSh("ensure").code).toBe(0);
  const r = t.bunSh("path");
  expect(r.code).toBe(0);
  expect(r.stdout.trim()).toBe(join(t.dir, "path-bin", "bun"));
  expect(existsSync(t.tools)).toBe(false);
});

test.skipIf(isWindows)("another version first on PATH is not the pin: path refuses, ensure installs the pin", () => {
  const t = setup({ pathBunVersion: "1.0.0" });
  const before = t.bunSh("path");
  expect(before.code).toBe(1);
  expect(before.stderr).toContain(`bun ${PIN} (pinned in .tool-versions) is not installed; run 'just bootstrap'`);

  publishRelease(t.dir);
  const ensure = t.bunSh("ensure");
  expect(ensure.code).toBe(0);
  expect(ensure.stderr).toContain(`installing bun ${PIN}`);
  const cached = join(t.tools, `bun-${PIN}`, "bin", "bun");
  expect(t.bunSh("path").stdout.trim()).toBe(cached);
  expect(ok(cached, ["--version"])).toBe(PIN);
  // Idempotent: a second ensure finds the cached pin and installs nothing.
  expect(t.bunSh("ensure").stderr).toBe("");
});

test.skipIf(isWindows)("a checksum mismatch refuses to install", () => {
  const t = setup();
  publishRelease(t.dir, { corrupt: true });
  const r = t.bunSh("ensure");
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("checksum mismatch");
  expect(existsSync(join(t.tools, `bun-${PIN}`, "bin", "bun"))).toBe(false);
});

test.skipIf(isWindows)("a missing release is a download failure with the next action", () => {
  const t = setup();
  const r = t.bunSh("ensure");
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("re-run 'just bootstrap'");
});

test.skipIf(isWindows)("a malformed pin is refused", () => {
  const t = setup({ pin: "latest" });
  const r = t.bunSh("path");
  expect(r.code).toBe(1);
  expect(r.stderr).toContain(".tool-versions must pin bun as 'bun X.Y.Z'");
});

/** A PATH dir holding only the named system tools, as symlinks. */
function toolsOnly(dir, names) {
  const only = join(dir, "only-bin");
  mkdirSync(only);
  for (const name of names) {
    const real = ["/usr/bin", "/bin"].map((d) => join(d, name)).find((p) => existsSync(p));
    if (real) symlinkSync(real, join(only, name));
  }
  return [only];
}
const BASICS = ["bash", "sh", "awk", "grep", "mktemp", "rm", "mkdir", "mv", "chmod", "uname", "dirname", "cat", "sha256sum", "shasum", "env"];

test.skipIf(isWindows)("an unknown mode prints the usage", () => {
  const r = setup().bunSh("sideload");
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("usage: scripts/bun.sh ensure | path");
});

test.skipIf(isWindows)("a release without SHASUMS256.txt is not installed", () => {
  const t = setup();
  publishRelease(t.dir, { noSums: true });
  const r = t.bunSh("ensure");
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("SHASUMS256.txt failed; check your network and re-run 'just bootstrap'");
  expect(existsSync(join(t.tools, `bun-${PIN}`, "bin", "bun"))).toBe(false);
});

test.skipIf(isWindows)("an archive without the bun binary is refused with the fallback", () => {
  const t = setup();
  publishRelease(t.dir, { empty: true });
  const r = t.bunSh("ensure");
  expect(r.code).toBe(1);
  expect(r.stderr).toContain(`holds no ${ASSET}/bun`);
});

test.skipIf(isWindows)("a downloaded bun that is not the pin is refused", () => {
  const t = setup();
  publishRelease(t.dir, { reports: "1.2.3" });
  const r = t.bunSh("ensure");
  expect(r.code).toBe(1);
  expect(r.stderr).toContain(`installed bun does not report ${PIN}; remove`);
});

test.skipIf(isWindows)("a host with no bun build names what to do", () => {
  const t = setup();
  writeFileSync(join(t.pathDir, "uname"), '#!/bin/sh\n[ "$1" = "-s" ] && echo Plan9 || echo mips\n');
  chmodSync(join(t.pathDir, "uname"), 0o755);
  const r = t.bunSh("ensure");
  expect(r.code).toBe(1);
  expect(r.stderr).toContain(`no bun build for Plan9/mips here; install bun ${PIN} yourself`);
});

test.skipIf(isWindows)("missing curl or unzip names the package to install", () => {
  for (const missing of ["curl", "unzip"]) {
    const s = scratch();
    cleanups.push(s.cleanup);
    const keep = [...BASICS, "curl", "unzip"].filter((n) => n !== missing);
    const t = setup({ systemDirs: toolsOnly(s.dir, keep) });
    const r = t.bunSh("ensure");
    expect(r.code).toBe(1);
    expect(r.stderr).toContain(`${missing} is required to install bun ${PIN}; install it with your package manager`);
  }
});

test.skipIf(isWindows)("on Windows it asks for the pin on PATH instead of installing", () => {
  const t = setup({ extraEnv: { OS: "Windows_NT" } });
  const r = t.bunSh("ensure");
  expect(r.code).toBe(1);
  expect(r.stderr).toContain(`install bun ${PIN} first on PATH`);
});

test.skipIf(isWindows)("a checksum-valid archive unzip cannot read is refused with the next action", () => {
  const t = setup();
  publishRelease(t.dir, { garbage: true });
  const r = t.bunSh("ensure");
  expect(r.code).toBe(1);
  expect(r.stderr).toContain(`could not unpack ${ASSET}.zip`);
});

test.skipIf(isWindows || process.getuid?.() === 0)("an unwritable tool cache names the override", () => {
  const t = setup();
  publishRelease(t.dir);
  mkdirSync(t.tools);
  chmodSync(t.tools, 0o555);
  const r = t.bunSh("ensure");
  chmodSync(t.tools, 0o755);
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("make it writable or point GH_SECRETS_TOOLS_DIR elsewhere");
});
