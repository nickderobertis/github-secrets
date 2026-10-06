// scripts/msrv.sh with stand-ins for cargo and rustup: the MSRV comes from cargo
// metadata, must agree with clippy.toml, its toolchain is installed only when
// missing, and the check runs on exactly that toolchain.
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

/** `rustVersion` is what cargo metadata reports; `installed` whether rustup has it. */
function setup({ rustVersion = "1.86", clippy = "1.86", installed = true, installFails = false, checkExit = 0 } = {}) {
  const s = scratch();
  cleanups.push(s.cleanup);
  const repo = join(s.dir, "repo");
  mkdirSync(join(repo, "scripts"), { recursive: true });
  copyFileSync(join(REPO, "scripts/msrv.sh"), join(repo, "scripts/msrv.sh"));
  writeFileSync(join(repo, "clippy.toml"), `msrv = "${clippy}"\n`);
  const bin = join(s.dir, "bin");
  mkdirSync(bin);
  const calls = join(s.dir, "calls");
  const metadata = JSON.stringify({ packages: [{ name: "gh-secrets", rust_version: rustVersion }] });
  exe(join(bin, "cargo"), `echo "cargo $*" >> "${calls}"\n[ "$1" = metadata ] && { echo '${metadata}'; exit 0; }\nexit ${checkExit}`);
  exe(
    join(bin, "rustup"),
    `echo "rustup $*" >> "${calls}"\ncase "$1" in\n  run) exit ${installed ? 0 : 1} ;;\n  toolchain) exit ${installFails ? 1 : 0} ;;\nesac`,
  );
  const msrv = () => run("bash", ["scripts/msrv.sh"], { cwd: repo, env: { ...process.env, PATH: [bin, "/usr/bin", "/bin"].join(":") } });
  const log = () => (existsSync(calls) ? readFileSync(calls, "utf8").trim().split("\n") : []);
  return { msrv, log };
}

test.skipIf(isWindows)("checks the crate on the declared MSRV toolchain, installing nothing when present", () => {
  const t = setup();
  const r = t.msrv();
  expect(r.code).toBe(0);
  expect(t.log()).toContain("cargo +1.86 check -p gh-secrets --locked --all-targets --all-features");
  expect(t.log().some((l) => l.startsWith("rustup toolchain"))).toBe(false);
});

test.skipIf(isWindows)("a missing MSRV toolchain is installed (minimal profile) first", () => {
  const t = setup({ installed: false });
  expect(t.msrv().code).toBe(0);
  const log = t.log();
  expect(log.indexOf("rustup toolchain install 1.86 --profile minimal")).toBeLessThan(log.indexOf("cargo +1.86 check -p gh-secrets --locked --all-targets --all-features"));
});

test.skipIf(isWindows)("cargo check's failure is the script's failure", () => {
  expect(setup({ checkExit: 101 }).msrv().code).toBe(101);
});

test.skipIf(isWindows)("a clippy msrv that disagrees is refused before any check", () => {
  const t = setup({ clippy: "1.85" });
  const r = t.msrv();
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("clippy.toml msrv '1.85' differs from the crate's rust-version '1.86'");
  expect(t.log().some((l) => l.includes("check"))).toBe(false);
});

test.skipIf(isWindows)("an unreadable MSRV or a failed install names the next action", () => {
  const unread = setup({ rustVersion: "" }).msrv();
  expect(unread.code).toBe(1);
  expect(unread.stderr).toContain("could not read gh-secrets' rust-version");
  const failed = setup({ installed: false, installFails: true }).msrv();
  expect(failed.code).toBe(1);
  expect(failed.stderr).toContain("could not install Rust 1.86 via rustup");
});
