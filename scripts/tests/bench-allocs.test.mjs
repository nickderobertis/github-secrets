// scripts/bench-allocs.sh with a stand-in `cargo`: the report file exists only
// when the bench succeeded, so the Performance report never shows a failed run
// as a result.
import { afterEach, expect, test } from "bun:test";
import { chmodSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { REPO, run, scratch } from "./helpers.mjs";

const isWindows = process.platform === "win32";
let cleanups = [];
afterEach(() => {
  for (const c of cleanups) c();
  cleanups = [];
});

function withCargo(body, { existingReport = null, out = null } = {}) {
  const s = scratch();
  cleanups.push(s.cleanup);
  const bin = join(s.dir, "bin");
  mkdirSync(bin);
  writeFileSync(join(bin, "cargo"), `#!/bin/sh\n${body}\n`);
  chmodSync(join(bin, "cargo"), 0o755);
  out ??= join(s.dir, "bench");
  if (existingReport !== null) {
    mkdirSync(out, { recursive: true });
    writeFileSync(join(out, "allocs.md"), existingReport);
  }
  const r = run("bash", [join(REPO, "scripts/bench-allocs.sh")], {
    env: { ...process.env, PATH: [bin, "/usr/bin", "/bin"].join(":"), BENCH_OUT: out },
  });
  return { r, report: join(out, "allocs.md") };
}

test.skipIf(isWindows)("a successful bench writes the report and names it in one line", () => {
  const { r, report } = withCargo('echo "| case | calls | bytes |"');
  expect(r.code).toBe(0);
  expect(r.stdout).toBe("");
  expect(r.stderr.trim()).toBe(`bench-allocs: wrote ${report}`);
  expect(readFileSync(report, "utf8").trim()).toBe("| case | calls | bytes |");
});

test.skipIf(isWindows)("a failing bench leaves an earlier report untouched", () => {
  const { r, report } = withCargo("exit 101", { existingReport: "| last good run |\n" });
  expect(r.code).toBe(1);
  expect(readFileSync(report, "utf8")).toBe("| last good run |\n");
});

test.skipIf(isWindows || process.getuid?.() === 0)("an unwritable output directory names the fix", () => {
  const s = scratch();
  cleanups.push(s.cleanup);
  const locked = join(s.dir, "locked");
  mkdirSync(locked);
  chmodSync(locked, 0o555);
  const { r } = withCargo('echo "| x |"', { out: join(locked, "bench") });
  chmodSync(locked, 0o755);
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("make it writable or set BENCH_OUT");
});

test.skipIf(isWindows)("a failing bench fails the script and leaves no report", () => {
  const { r, report } = withCargo('echo "| partial |"; echo "error: bench panicked" >&2; exit 101');
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("the engine_allocs bench failed (its output is above); no report was written");
  expect(existsSync(report)).toBe(false);
});

test.skipIf(isWindows)("a failing bench's own output is surfaced", () => {
  const { r } = withCargo('echo "panicked at engine_allocs.rs:42"; exit 101');
  expect(r.stderr).toContain("panicked at engine_allocs.rs:42");
});

test.skipIf(isWindows || process.getuid?.() === 0)("an existing but read-only output directory names the fix", () => {
  const s = scratch();
  cleanups.push(s.cleanup);
  const out = join(s.dir, "ro");
  mkdirSync(out);
  chmodSync(out, 0o555);
  const { r } = withCargo('echo "| x |"', { out });
  chmodSync(out, 0o755);
  expect(r.code).toBe(1);
  expect(r.stderr).toContain(`cannot write in ${out}; make it writable or set BENCH_OUT`);
});

test.skipIf(isWindows)("a directory where the report goes is refused, not written into", () => {
  const s = scratch();
  cleanups.push(s.cleanup);
  const out = join(s.dir, "bench");
  mkdirSync(join(out, "allocs.md"), { recursive: true });
  const { r } = withCargo('echo "| x |"', { out });
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("allocs.md is a directory; remove it");
});
