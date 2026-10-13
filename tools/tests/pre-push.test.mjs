// Real local pushes with both a fresh clone and the legacy hooksPath setting.
// Git's trace records child execution without replacing any tool on PATH.
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { REPO, gitRepo, ok, run } from "../../scripts/tests/helpers.mjs";

for (const legacy of [false, true]) {
  describe(`ordinary pushes have no duplicate checks (${legacy ? "existing" : "fresh"} clone)`, () => {
    let dir, remote, git, commit, baseline, gallery, trace;
    beforeAll(() => {
      dir = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "ghs-push-"));
      remote = join(dir, "remote.git");
      ok("git", ["init", "-q", "--bare", "-b", "master", remote]);
      ({ git, commit } = gitRepo(dir));
      git("remote", "add", "origin", remote);
      if (legacy) git("config", "core.hooksPath", ".githooks");
      const justfile = readFileSync(join(REPO, "justfile"), "utf8");
      writeFileSync(join(dir, "justfile"), justfile);
      baseline = join(dir, "baseline.png");
      gallery = join(dir, "gallery.html");
      writeFileSync(baseline, "baseline sentinel");
      writeFileSync(gallery, "gallery sentinel");
      commit("initial");
      trace = join(dir, "push.trace");
    });
    afterAll(() => {
      if (dir) rmSync(dir, { recursive: true, force: true });
    });
    const push = () => {
      const result = run("git", ["push", "origin", "master"], {
        cwd: dir,
        env: { ...process.env, GIT_TRACE: trace },
      });
      expect(result.code).toBe(0);
      expect(ok("git", ["--git-dir", remote, "rev-parse", "master"])).toBe(git("rev-parse", "HEAD"));
    };
    test("first push", () => {
      push();
    });
    test("changed guarded path", () => {
      writeFileSync(join(dir, "llmlint.yml"), "changed guarded path\n");
      commit("change validator config");
      push();
    });
    test("up-to-date push", () => {
      push();
    });
    // Bootstrap no longer installs any hooks. Inspect its real command expansion
    // without provisioning unrelated Rust and network dependencies in the fixture.
    for (let rerun = 0; rerun < 2; rerun++) {
      test(`bootstrap dry run ${rerun + 1} followed by push`, () => {
        const expansion = run("just", ["--dry-run", "bootstrap"], { cwd: dir });
        expect(expansion.code).toBe(0);
        expect(expansion.stdout + expansion.stderr).not.toMatch(/git config|hooksPath|pre-push/);
        push();
      });
    }
    test("pushes preserve hook settings and artifacts without running checks", () => {
      expect(run("git", ["config", "--get", "core.hooksPath"], { cwd: dir }).stdout.trim()).toBe(legacy ? ".githooks" : "");
      expect(readFileSync(trace, "utf8")).not.toMatch(/pre-push|llmlint|capture/);
      expect(readFileSync(baseline, "utf8")).toBe("baseline sentinel");
      expect(readFileSync(gallery, "utf8")).toBe("gallery sentinel");
    });
    test("remote refusal preserves refs", () => {
      git("remote", "set-url", "origin", join(dir, "missing.git"));
      expect(run("git", ["push", "origin", "master"], { cwd: dir }).code).not.toBe(0);
      expect(ok("git", ["--git-dir", remote, "rev-parse", "master"])).toBe(git("rev-parse", "HEAD"));
    });
  });
}

test("the obsolete repository hook is absent", () => {
  expect(() => readFileSync(join(REPO, ".githooks/pre-push"))).toThrow();
});
