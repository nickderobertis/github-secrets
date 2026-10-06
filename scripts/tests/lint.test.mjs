// scripts/lint.sh, the scripts project's lint target: the real scripts pass, and
// a syntax error (bash) or a bashism in a POSIX sh installer fails it, naming the
// file. The target is a script because Nx runs inline commands through cmd.exe
// on Windows; the Windows check leg runs it through Nx for real.
import { afterEach, expect, test } from "bun:test";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { REPO, run, scratch } from "./helpers.mjs";

const isWindows = process.platform === "win32";
let cleanups = [];
afterEach(() => {
  for (const c of cleanups) c();
  cleanups = [];
});

function scriptsDir(files) {
  const s = scratch();
  cleanups.push(s.cleanup);
  const valid = { "a.sh": "echo ok\n", nx: "echo ok\n", "install.sh": "echo ok\n", "install-nextest.sh": "echo ok\n" };
  for (const [name, body] of Object.entries({ ...valid, ...files })) writeFileSync(join(s.dir, name), body);
  return s.dir;
}

const lint = (...args) => run("bash", [join(REPO, "scripts/lint.sh"), ...args]);

test.skipIf(isWindows)("the committed scripts pass quietly", () => {
  const r = lint();
  expect(r.code).toBe(0);
  expect(r.stdout + r.stderr).toBe("");
});

test.skipIf(isWindows)("a bash syntax error fails, naming the file", () => {
  const dir = scriptsDir({ "broken.sh": "if true; then\n" });
  const r = lint(dir);
  expect(r.code).toBe(1);
  expect(r.stderr).toContain(`lint: ${dir}/broken.sh has a bash syntax error`);
});

test.skipIf(isWindows)("the nx wrapper is checked too", () => {
  const dir = scriptsDir({ nx: "for x in; do\n" });
  const r = lint(dir);
  expect(r.code).toBe(1);
  expect(r.stderr).toContain(`lint: ${dir}/nx has a bash syntax error`);
});

test.skipIf(isWindows)("a bashism in a POSIX sh installer fails, naming the file", () => {
  // Valid bash, not valid POSIX sh: `sh -n` (dash on Linux, bash --posix on macOS) refuses it.
  const dir = scriptsDir({ "install.sh": "f() { cat <(echo x); }\n" });
  const r = lint(dir);
  expect(r.code).toBe(1);
  expect(r.stderr).toContain(`lint: ${dir}/install.sh is not POSIX sh`);
});
