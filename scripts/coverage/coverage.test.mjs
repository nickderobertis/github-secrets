// scripts/coverage/coverage.sh end to end on a scratch Cargo workspace shaped like this
// one: a `gh-secrets` crate (lib + bin) and an e2e member crate whose test spawns
// the binary. Real cargo-llvm-cov and nextest run; the cases show that the
// spawned binary's profiles join the union (the floor passes only with them),
// that sources under tests/ are left out, and that an untested product line
// fails the floor.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

// Self-contained on purpose: importing the scripts project's test helpers would
// make this (slow) project depend on that one, and every script edit would rerun it.
const REPO = resolve(import.meta.dir, "../..");
const run = (cmd, args, opts = {}) => {
  const r = spawnSync(cmd, args, { encoding: "utf8", ...opts });
  if (r.error) throw r.error;
  return { code: r.status, stdout: r.stdout ?? "", stderr: r.stderr ?? "" };
};
const ok = (cmd, args, opts = {}) => {
  const r = run(cmd, args, opts);
  if (r.code !== 0) throw new Error(`${cmd} ${args.join(" ")} exited ${r.code}: ${r.stderr}`);
  return r.stdout.trim();
};
const scratch = () => {
  const dir = mkdtempSync(join(tmpdir(), "ghs-coverage-"));
  return { dir, cleanup: () => rmSync(dir, { recursive: true, force: true }) };
};

const skip = process.platform === "win32" || run("cargo", ["llvm-cov", "--version"]).code !== 0;
let s, ws;

const write = (path, text) => {
  mkdirSync(join(ws, path, ".."), { recursive: true });
  writeFileSync(join(ws, path), text);
};
const cov = (...args) => run("bash", ["scripts/coverage/coverage.sh", ...args], { cwd: ws, env: { ...process.env, CARGO_TARGET_DIR: join(ws, "target") } });

beforeAll(() => {
  if (skip) return;
  s = scratch();
  ws = join(s.dir, "ws");
  write("Cargo.toml", `[package]\nname = "gh-secrets"\nversion = "0.1.0"\nedition = "2021"\nautotests = false\n\n[workspace]\nmembers = ["tests/e2e"]\n`);
  // greet() is unit-tested; banner() runs only when the binary does.
  write("src/lib.rs", `pub fn greet() -> &'static str {\n    "hi"\n}\n\npub fn banner() -> String {\n    let mut s = String::from("banner:");\n    s.push_str(greet());\n    s\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn greets() {\n        assert_eq!(super::greet(), "hi");\n    }\n}\n`);
  write("src/main.rs", `fn main() {\n    println!("{}", gh_secrets::banner());\n}\n`);
  write("tests/e2e/Cargo.toml", `[package]\nname = "app-e2e"\nversion = "0.1.0"\nedition = "2021"\n`);
  // Never called: proves sources under tests/ stay out of the report.
  write("tests/e2e/src/lib.rs", `pub fn unused_helper(x: u32) -> u32 {\n    let y = x + 1;\n    y * 2\n}\n`);
  write("tests/e2e/tests/run.rs", `#[test]\nfn binary_prints_banner() {\n    let exe = std::env::current_exe().unwrap();\n    let bin = exe.parent().unwrap().parent().unwrap().join("gh-secrets");\n    let out = std::process::Command::new(bin).output().unwrap();\n    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "banner:hi");\n}\n`);
  mkdirSync(join(ws, "scripts/coverage"), { recursive: true });
  copyFileSync(join(REPO, "scripts/coverage/coverage.sh"), join(ws, "scripts/coverage/coverage.sh"));
  ok("cargo", ["generate-lockfile", "--offline"], { cwd: ws });
});
afterAll(() => s?.cleanup());

test.skipIf(skip)("the floor needs the spawned binary's profiles, ignores tests/, and fails on an untested line", () => {
  expect(cov("clear").code).toBe(0);
  expect(cov("test", "gh-secrets").code).toBe(0);
  // Unit tests alone never run banner()/main(): below the floor.
  const partial = cov("report");
  expect(partial.code).toBe(1);
  expect(partial.stderr).toContain("below 95% line coverage");

  // The e2e crate spawns the instrumented binary; its profiles complete the union.
  // 100% (not merely >= 95%) also shows tests/e2e/src/lib.rs's never-called
  // helper is outside the report.
  const e2e = cov("test", "app-e2e");
  expect(e2e.code).toBe(0);
  const full = cov("report");
  expect(full.stderr).toContain("coverage: 100.00% lines covered (floor 95%)");
  expect(full.code).toBe(0);

  // An untested product function drops the crate below the floor.
  write("src/lib.rs", `${"pub fn untested(x: u32) -> u32 {\n" + "    let a = x + 1;\n".repeat(30) + "    a\n}\n"}` + readFileSync(join(ws, "src/lib.rs"), "utf8"));
  expect(cov("clear").code).toBe(0);
  expect(cov("test", "gh-secrets").code).toBe(0);
  expect(cov("test", "app-e2e").code).toBe(0);
  const dropped = cov("report");
  expect(dropped.code).toBe(1);
  expect(dropped.stderr).toContain("lib.rs");
  expect(dropped.stderr).not.toContain("e2e/src/lib.rs");
}, 600_000);

test.skipIf(skip)("an unknown crate is refused with the members to choose from", () => {
  const r = cov("test", "no-such-crate");
  expect(r.code).toBe(2);
  expect(r.stderr).toContain("is not a member of this Cargo workspace; pass one of: ");
  expect(r.stderr).toContain("gh-secrets");
});
