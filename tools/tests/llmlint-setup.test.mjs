// The llmlint tier's installers, run for real with stand-ins for the tools they
// drive: scripts/setup-llmlint.sh with a fake `uv` (which "installs" a fake
// llmlint into $HOME/.local/bin), and scripts/session-setup.sh's hand-off to it.
// Both are best-effort session-startup installers: every path must exit 0 and
// say what happened.
import { afterEach, expect, test } from "bun:test";
import { chmodSync, copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { REPO, run, scratch } from "./helpers.mjs";

const isWindows = process.platform === "win32";
let cleanups = [];
afterEach(() => {
  for (const c of cleanups) c();
  cleanups = [];
});

function exe(path, body) {
  writeFileSync(path, `#!/bin/sh\n${body}\n`);
  chmodSync(path, 0o755);
}

function setupLlmlint({ uv = "ok", doctor = 0 } = {}) {
  const s = scratch();
  cleanups.push(s.cleanup);
  const bin = join(s.dir, "bin");
  const home = join(s.dir, "home");
  mkdirSync(bin);
  mkdirSync(home);
  const uvCalls = join(s.dir, "uv-calls");
  const llmlintBody = `[ "$1" = "--version" ] && { echo "llmlint 0.9.9"; exit 0; }\n[ "$1" = "doctor" ] && exit ${doctor}\nexit 0`;
  if (uv === "ok") {
    exe(join(bin, "uv"), `echo "$*" >> "${uvCalls}"\nmkdir -p "$HOME/.local/bin"\nprintf '%s\\n' '#!/bin/sh' '${llmlintBody.replaceAll("\n", "' '")}' > "$HOME/.local/bin/llmlint"\nchmod +x "$HOME/.local/bin/llmlint"`);
  } else if (uv === "fails") {
    exe(join(bin, "uv"), `echo "$*" >> "${uvCalls}"\necho "error: no network" >&2\nexit 1`);
  }
  const envFile = join(s.dir, "claude-env");
  const go = (extraEnv = {}) =>
    run("bash", [join(REPO, "scripts/setup-llmlint.sh")], {
      env: { ...process.env, HOME: home, PATH: [bin, "/usr/bin", "/bin"].join(":"), CLAUDE_ENV_FILE: "", ...extraEnv },
    });
  const calls = () => (existsSync(uvCalls) ? readFileSync(uvCalls, "utf8").trim() : "");
  return { home, envFile, go, calls };
}

test.skipIf(isWindows)("installs llmlint-cli at the floor via uv tool, checks doctor, persists PATH into the session", () => {
  const t = setupLlmlint();
  const r = t.go({ CLAUDE_ENV_FILE: t.envFile });
  expect(r.code).toBe(0);
  expect(t.calls()).toBe("tool install --upgrade llmlint-cli>=0.3.23");
  expect(r.stderr).toContain("setup-llmlint: ready (llmlint: llmlint 0.9.9)");
  expect(readFileSync(t.envFile, "utf8")).toContain(`export PATH=${join(t.home, ".local/bin")}:`);
});

test.skipIf(isWindows)("outside a session it installs but writes no env", () => {
  const t = setupLlmlint();
  const r = t.go();
  expect(r.code).toBe(0);
  expect(r.stderr).toContain("no CLAUDE_ENV_FILE (not a session); skipping env");
  expect(existsSync(t.envFile)).toBe(false);
});

test.skipIf(isWindows)("without uv it says how to get it and still exits 0", () => {
  const t = setupLlmlint({ uv: "absent" });
  const r = t.go();
  expect(r.code).toBe(0);
  expect(r.stderr).toContain("uv not found; cannot install llmlint (install uv:");
  expect(r.stderr).toContain("setup-llmlint: llmlint not installed");
});

test.skipIf(isWindows)("a failed install is logged and startup continues", () => {
  const t = setupLlmlint({ uv: "fails" });
  const r = t.go();
  expect(r.code).toBe(0);
  expect(r.stderr).toContain("llmlint-cli install failed (continuing)");
});

test.skipIf(isWindows)("a failing doctor is reported, not fatal", () => {
  const t = setupLlmlint({ doctor: 1 });
  const r = t.go();
  expect(r.code).toBe(0);
  expect(r.stderr).toContain("llmlint doctor reported an issue");
});

test.skipIf(isWindows)("session-setup hands off to setup-llmlint and survives its failure", () => {
  const s = scratch();
  cleanups.push(s.cleanup);
  const scripts = join(s.dir, "scripts");
  const bin = join(s.dir, "bin");
  mkdirSync(scripts);
  mkdirSync(bin);
  copyFileSync(join(REPO, "scripts/session-setup.sh"), join(scripts, "session-setup.sh"));
  const handoff = join(s.dir, "handoff");
  exe(join(scripts, "setup-llmlint.sh"), `echo called > "${handoff}"\nexit 3`);
  // The toolchain session-setup checks for, already present.
  exe(join(bin, "rustup"), "exit 0");
  exe(join(bin, "just"), 'echo "just 1.51.0"');
  exe(join(bin, "cargo-nextest"), 'echo "cargo-nextest 0.9.0"');
  exe(join(bin, "cargo"), "exit 0");
  const r = run("bash", [join(scripts, "session-setup.sh")], {
    env: { ...process.env, HOME: join(s.dir, "home"), CARGO_HOME: join(s.dir, "cargo"), PATH: [bin, "/usr/bin", "/bin"].join(":") },
  });
  expect(r.code).toBe(0);
  expect(readFileSync(handoff, "utf8").trim()).toBe("called");
  expect(r.stderr).toContain("setup-llmlint.sh exited non-zero (its log is above); retry with 'just setup-llmlint'");
});
