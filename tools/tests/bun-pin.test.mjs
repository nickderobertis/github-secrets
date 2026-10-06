// scripts/bun.sh — the bun pin's resolver and installer — run for real in a
// scratch checkout with its own .tool-versions pin. The download path is driven
// offline: GH_SECRETS_BUN_DOWNLOAD_BASE points curl at a file:// "release" laid
// out exactly like bun's GitHub release (bun-v<ver>/<asset>.zip + SHASUMS256.txt).
import { afterEach, expect, test } from "bun:test";
import { chmodSync, copyFileSync, existsSync, mkdirSync, writeFileSync } from "node:fs";
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
function setup({ pin = PIN, pathBunVersion = null } = {}) {
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
    PATH: [pathDir, "/usr/bin", "/bin"].join(":"),
    GH_SECRETS_TOOLS_DIR: tools,
    GH_SECRETS_BUN_DOWNLOAD_BASE: `file://${join(s.dir, "release")}`,
  };
  const bunSh = (mode) => run("bash", ["scripts/bun.sh", mode], { cwd: repo, env });
  return { dir: s.dir, repo, tools, bunSh };
}

/** Lay out a fake bun release; `corrupt` publishes a wrong checksum. */
function publishRelease(dir, { corrupt = false } = {}) {
  const rel = join(dir, "release", `bun-v${PIN}`);
  const stage = join(dir, "stage", ASSET);
  mkdirSync(rel, { recursive: true });
  mkdirSync(stage, { recursive: true });
  fakeBun(join(stage, "bun"), PIN);
  ok("python3", ["-I", "-c", `import shutil; shutil.make_archive(${JSON.stringify(join(rel, ASSET))}, "zip", ${JSON.stringify(join(dir, "stage"))}, ${JSON.stringify(ASSET)})`]);
  const sum = ok("bash", ["-c", `(sha256sum "${ASSET}.zip" 2>/dev/null || shasum -a 256 "${ASSET}.zip") | awk '{print $1}'`], { cwd: rel });
  writeFileSync(join(rel, "SHASUMS256.txt"), `${corrupt ? "0".repeat(64) : sum}  ${ASSET}.zip\n`);
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
