// Real local pushes with both a fresh clone and the legacy hooksPath setting.
// Git's trace records child execution without replacing any tool on PATH.
import { afterEach, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { REPO, gitRepo, ok, run } from "../../scripts/tests/helpers.mjs";

let cleanups = [];
afterEach(() => {
  for (const cleanup of cleanups) cleanup();
  cleanups = [];
});

for (const legacy of [false, true]) {
  test(`ordinary pushes have no duplicate checks (${legacy ? "existing" : "fresh"} clone)`, () => {
    const dir = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "ghs-push-"));
    cleanups.push(() => rmSync(dir, { recursive: true, force: true }));
    const remote = join(dir, "remote.git");
    ok("git", ["init", "-q", "--bare", "-b", "master", remote]);
    const { git, commit } = gitRepo(dir);
    git("remote", "add", "origin", remote);
    if (legacy) git("config", "core.hooksPath", ".githooks");
    const justfile = readFileSync(join(REPO, "justfile"), "utf8");
    writeFileSync(join(dir, "justfile"), justfile);
    const baseline = join(dir, "baseline.png");
    const gallery = join(dir, "gallery.html");
    writeFileSync(baseline, "baseline sentinel");
    writeFileSync(gallery, "gallery sentinel");
    commit("initial");
    const trace = join(dir, "push.trace");
    const push = () => {
      const result = run("git", ["push", "origin", "master"], {
        cwd: dir,
        env: { ...process.env, GIT_TRACE: trace },
      });
      expect(result.code).toBe(0);
      expect(ok("git", ["--git-dir", remote, "rev-parse", "master"])).toBe(git("rev-parse", "HEAD"));
    };
    push();
    writeFileSync(join(dir, "llmlint.yml"), "changed guarded path\n");
    commit("change validator config");
    push();
    push();
    // Bootstrap no longer installs any hooks. Inspect its real command expansion
    // without provisioning unrelated Rust and network dependencies in the fixture.
    for (let rerun = 0; rerun < 2; rerun++) {
      const expansion = run("just", ["--dry-run", "bootstrap"], { cwd: dir });
      expect(expansion.code).toBe(0);
      expect(expansion.stdout + expansion.stderr).not.toMatch(/git config|hooksPath|pre-push/);
      push();
    }
    expect(run("git", ["config", "--get", "core.hooksPath"], { cwd: dir }).stdout.trim()).toBe(legacy ? ".githooks" : "");
    expect(readFileSync(trace, "utf8")).not.toMatch(/pre-push|llmlint|capture/);
    expect(readFileSync(baseline, "utf8")).toBe("baseline sentinel");
    expect(readFileSync(gallery, "utf8")).toBe("gallery sentinel");
    // A genuine remote refusal still fails and leaves its refs unchanged.
    git("remote", "set-url", "origin", join(dir, "missing.git"));
    expect(run("git", ["push", "origin", "master"], { cwd: dir }).code).not.toBe(0);
    expect(ok("git", ["--git-dir", remote, "rev-parse", "master"])).toBe(git("rev-parse", "HEAD"));
  });
}

test("the obsolete repository hook is absent", () => {
  expect(() => readFileSync(join(REPO, ".githooks/pre-push"))).toThrow();
});
