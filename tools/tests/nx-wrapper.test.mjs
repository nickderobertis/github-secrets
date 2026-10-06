// scripts/nx — the wrapper every gate recipe runs Nx through — in a scratch
// checkout. The pinned bun is a stand-in on PATH that records its calls (and
// "installs" by writing the node_modules/.bin/nx stub), so each case shows what
// the wrapper did before handing over to Nx, and what Nx was handed.
import { afterEach, expect, test } from "bun:test";
import { chmodSync, copyFileSync, existsSync, mkdirSync, readFileSync, utimesSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { REPO, run, scratch } from "./helpers.mjs";

const isWindows = process.platform === "win32";
const PIN = "9.9.9";
let cleanups = [];
afterEach(() => {
  for (const c of cleanups) c();
  cleanups = [];
});

function setup({ installFails = false, withNode = true } = {}) {
  const s = scratch();
  cleanups.push(s.cleanup);
  const repo = join(s.dir, "repo");
  mkdirSync(join(repo, "scripts"), { recursive: true });
  for (const f of ["scripts/nx", "scripts/bun.sh"]) copyFileSync(join(REPO, f), join(repo, f));
  writeFileSync(join(repo, ".tool-versions"), `bun ${PIN}\n`);
  writeFileSync(join(repo, "package.json"), "{}\n");
  writeFileSync(join(repo, "bun.lock"), "{}\n");
  const bin = join(s.dir, "bin");
  mkdirSync(bin);
  const calls = join(s.dir, "bun-calls");
  const nxStub = `#!/bin/sh\\necho "nx-args: $*"\\necho "nx-bun: $(command -v bun)"\\necho "nx-daemon: $NX_DAEMON"\\n`;
  writeFileSync(
    join(bin, "bun"),
    `#!/bin/sh\n[ "$1" = "--version" ] && { echo ${PIN}; exit 0; }\necho "$*" >> "${calls}"\n` +
      (installFails
        ? `echo "error: lockfile had changes, but lockfile is frozen" >&2; exit 1\n`
        : `mkdir -p node_modules/.bin && printf '${nxStub}' > node_modules/.bin/nx && chmod +x node_modules/.bin/nx\n`),
  );
  chmodSync(join(bin, "bun"), 0o755);
  if (withNode) {
    writeFileSync(join(bin, "node"), "#!/bin/sh\nexit 0\n");
    chmodSync(join(bin, "node"), 0o755);
  }
  const env = { ...process.env, PATH: [bin, "/usr/bin", "/bin"].join(":"), GH_SECRETS_TOOLS_DIR: join(s.dir, "tools") };
  const nx = (...args) => run("bash", ["scripts/nx", ...args], { cwd: repo, env });
  const installs = () => (existsSync(calls) ? readFileSync(calls, "utf8").trim().split("\n").filter(Boolean) : []);
  return { repo, bin, nx, installs };
}

test.skipIf(isWindows)("a fresh checkout gets the locked install once, then Nx runs on the pinned bun without its daemon", () => {
  const t = setup();
  const first = t.nx("affected", "--base=abc", "-t", "lint");
  expect(first.code).toBe(0);
  expect(t.installs()).toEqual(["install --frozen-lockfile"]);
  expect(first.stdout).toContain("nx-args: affected --base=abc -t lint");
  expect(first.stdout).toContain(`nx-bun: ${join(t.bin, "bun")}`);
  expect(first.stdout).toContain("nx-daemon: false");

  expect(t.nx("run", "p:t").code).toBe(0);
  expect(t.installs()).toHaveLength(1);
});

test.skipIf(isWindows)("a bun.lock newer than the last install reinstalls", () => {
  const t = setup();
  expect(t.nx("--version").code).toBe(0);
  const future = new Date(Date.now() + 60_000);
  utimesSync(join(t.repo, "bun.lock"), future, future);
  expect(t.nx("--version").code).toBe(0);
  expect(t.installs()).toHaveLength(2);
});

test.skipIf(isWindows)("a failing locked install stops before Nx, with bun's reason and the next action", () => {
  const t = setup({ installFails: true });
  const r = t.nx("run", "p:t");
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("lockfile had changes");
  expect(r.stderr).toContain("nx: 'bun install --frozen-lockfile' failed");
  expect(r.stdout).not.toContain("nx-args");
});

test.skipIf(isWindows)("no Node on PATH is a precise error", () => {
  const t = setup({ withNode: false });
  const r = t.nx("run", "p:t");
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("nx: Nx runs on Node, which is not on PATH");
});

test.skipIf(isWindows)("without the pinned bun it points at bootstrap", () => {
  const t = setup();
  writeFileSync(join(t.bin, "bun"), "#!/bin/sh\necho 1.0.0\n");
  const r = t.nx("run", "p:t");
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("run 'just bootstrap'");
});
