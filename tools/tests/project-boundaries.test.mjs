// The boundary rule against a real (tiny) Cargo workspace: cargo metadata is the
// edge source, so the test builds one rather than feeding canned JSON.
import { afterEach, expect, test } from "bun:test";
import { cpSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { REPO, gitRepo, run, scratch } from "../../scripts/tests/helpers.mjs";

let cleanups = [];
afterEach(() => {
  for (const c of cleanups) c();
  cleanups = [];
});

function workspace({ appDevDep = "", e2eTags = ["type:e2e"] } = {}) {
  const { dir, cleanup } = scratch();
  cleanups.push(cleanup);
  const write = (path, text) => {
    mkdirSync(join(dir, path, ".."), { recursive: true });
    writeFileSync(join(dir, path), text);
  };
  write(
    "Cargo.toml",
    `[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n[dev-dependencies]\n${appDevDep}\n\n[workspace]\nmembers = ["e2e"]\n`,
  );
  write("src/lib.rs", "");
  write("e2e/Cargo.toml", `[package]\nname = "app-e2e"\nversion = "0.1.0"\nedition = "2021"\n`);
  write("e2e/src/lib.rs", "");
  write("project.json", JSON.stringify({ name: "app", tags: ["type:app"] }));
  write("e2e/project.json", JSON.stringify({ name: "app-e2e", tags: e2eTags, implicitDependencies: ["app"] }));
  mkdirSync(join(dir, "tools"), { recursive: true });
  cpSync(join(REPO, "tools/project-boundaries.json"), join(dir, "tools/project-boundaries.json"));
  gitRepo(dir);
  return dir;
}

const check = (dir) => run("bun", [join(REPO, "tools/check-project-boundaries.mjs"), "--root", dir]);

test("allowed edges pass quietly", () => {
  const r = check(workspace());
  expect(r.stderr).toBe("");
  expect(r.code).toBe(0);
});

test("the app crate depending on its e2e crate fails, naming both and the edge", () => {
  const r = check(workspace({ appDevDep: 'app-e2e = { path = "e2e" }' }));
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("app (type:app) may not depend on app-e2e (type:e2e)");
  expect(r.stderr).toContain("Cargo dev dependency app-e2e");
});

test("a project without exactly one type tag fails", () => {
  const r = check(workspace({ e2eTags: [] }));
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("app-e2e must carry exactly one type:* tag");
});

test("the real repository passes", () => {
  const r = check(REPO);
  expect(r.stderr).toBe("");
  expect(r.code).toBe(0);
});

test("implicit dependency globs expand the way Nx reads them", () => {
  const dir = workspace();
  // The e2e project claiming every other project (`*`) reaches app: allowed.
  writeFileSync(join(dir, "e2e/project.json"), JSON.stringify({ name: "app-e2e", tags: ["type:e2e"], implicitDependencies: ["*"] }));
  expect(check(dir).code).toBe(0);
  // The app claiming `*` reaches the e2e project: refused, naming the edge.
  writeFileSync(join(dir, "project.json"), JSON.stringify({ name: "app", tags: ["type:app"], implicitDependencies: ["*", "!nothing"] }));
  const r = check(dir);
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("app (type:app) may not depend on app-e2e (type:e2e) — found implicitDependencies");
});

test("malformed project definitions and policies are refused", () => {
  const dir = workspace();
  writeFileSync(join(dir, "e2e/project.json"), JSON.stringify({ name: "app", tags: ["type:e2e"] }));
  expect(check(dir).stderr).toContain("project name app is declared twice");
  writeFileSync(join(dir, "e2e/project.json"), JSON.stringify({ name: "app-e2e", tags: "type:e2e" }));
  expect(check(dir).stderr).toContain("tags and implicitDependencies must be arrays of strings");
  writeFileSync(join(dir, "tools/project-boundaries.json"), JSON.stringify({ depConstraints: [{ sourceTag: "app" }] }));
  expect(check(dir).stderr).toContain("malformed constraint");
});

test("a project.json holding JSON null, or a bad --root, is refused with a message", () => {
  const dir = workspace();
  writeFileSync(join(dir, "e2e/project.json"), "null");
  expect(check(dir).stderr).toContain("e2e/project.json must hold a JSON object");
  const r = run("bun", [join(REPO, "tools/check-project-boundaries.mjs"), "--root", join(dir, "missing")]);
  expect(r.code).toBe(2);
  expect(r.stderr).toContain("usage:");
});

test("tag: implicit dependencies resolve to the tagged projects", () => {
  const dir = workspace();
  writeFileSync(join(dir, "e2e/project.json"), JSON.stringify({ name: "app-e2e", tags: ["type:e2e", "suite"] }));
  writeFileSync(join(dir, "project.json"), JSON.stringify({ name: "app", tags: ["type:app"], implicitDependencies: ["tag:suite"] }));
  const r = check(dir);
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("app (type:app) may not depend on app-e2e (type:e2e) — found implicitDependencies");
});
