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

function withCargo(body) {
  const s = scratch();
  cleanups.push(s.cleanup);
  const bin = join(s.dir, "bin");
  mkdirSync(bin);
  writeFileSync(join(bin, "cargo"), `#!/bin/sh\n${body}\n`);
  chmodSync(join(bin, "cargo"), 0o755);
  const out = join(s.dir, "bench");
  const r = run("bash", [join(REPO, "scripts/bench-allocs.sh")], {
    env: { ...process.env, PATH: [bin, "/usr/bin", "/bin"].join(":"), BENCH_OUT: out },
  });
  return { r, report: join(out, "allocs.md") };
}

test.skipIf(isWindows)("a successful bench writes the report and echoes it", () => {
  const { r, report } = withCargo('echo "| case | calls | bytes |"');
  expect(r.code).toBe(0);
  expect(r.stdout.trim()).toBe("| case | calls | bytes |");
  expect(readFileSync(report, "utf8").trim()).toBe("| case | calls | bytes |");
});

test.skipIf(isWindows)("a failing bench fails the script and leaves no report", () => {
  const { r, report } = withCargo('echo "| partial |"; echo "error: bench panicked" >&2; exit 101');
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("the engine_allocs bench failed (above); no report was written");
  expect(existsSync(report)).toBe(false);
});
