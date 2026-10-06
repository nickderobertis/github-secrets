// The workflow contract checker against the real workflows, and against copies
// of them with one realistic regression each.
import { afterEach, expect, test } from "bun:test";
import { cpSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { checkContract } from "../check-workflow-contract.mjs";
import { REPO, scratch } from "../../scripts/tests/helpers.mjs";

let cleanups = [];
afterEach(() => {
  for (const c of cleanups) c();
  cleanups = [];
});

function mutated(file, from, to) {
  const s = scratch();
  cleanups.push(s.cleanup);
  cpSync(join(REPO, ".github"), join(s.dir, ".github"), { recursive: true });
  cpSync(join(REPO, "rust-toolchain.toml"), join(s.dir, "rust-toolchain.toml"));
  cpSync(join(REPO, "oneharness.toml"), join(s.dir, "oneharness.toml"));
  const path = join(s.dir, ".github/workflows", file);
  const text = readFileSync(path, "utf8");
  expect(text).toContain(from);
  writeFileSync(path, text.replace(from, to));
  return checkContract(s.dir);
}

test("the committed workflows satisfy the contract", () => {
  expect(checkContract(REPO)).toEqual([]);
});

test("a path filter on ci.yml's pull_request trigger is caught", () => {
  const errors = mutated("ci.yml", "  pull_request:\n", "  pull_request:\n    paths: ['src/**']\n");
  expect(errors.join("\n")).toContain("does not run on every pull request");
});

test("a fixed-context job gaining a needs edge on a non-fixed job is caught", () => {
  const errors = mutated("ci.yml", "  build:\n    needs: check\n", "  build:\n    needs: [check, install]\n");
  expect(errors.join("\n")).toContain("needs 'install', which reports none");
});

test("a new condition on a fixed-context job is caught", () => {
  const errors = mutated("ci.yml", "  build:\n    needs: check\n", "  build:\n    needs: check\n    if: github.event_name == 'push'\n");
  expect(errors.join("\n")).toContain("unrecognised condition");
});

test("renaming a fixed-context job is caught", () => {
  const errors = mutated("ci.yml", "  llmlint:\n", "  llm-judge:\n");
  expect(errors.join("\n")).toContain("no job reports the fixed context 'llmlint'");
});

test("packaging drift between CI's install job and the release is caught", () => {
  const errors = mutated("ci.yml", 'cp README.md LICENSE "$dist/"', 'cp README.md "$dist/"');
  expect(errors.join("\n")).toContain("'Package (unix)' differs");
});

test("a ci.yml job without the pinned just is caught", () => {
  const errors = mutated("ci.yml", "  msrv:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n\n      - name: Read tool pins (.tool-versions)\n        id: pins\n", "  msrv:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n\n      - name: Read tool pins (.tool-versions)\n        id: not-pins\n");
  expect(errors.join("\n")).toContain("ci.yml:msrv must install just at the .tool-versions pin");
});

test("a release target missing from rust-toolchain.toml is caught", () => {
  const s = scratch();
  cleanups.push(s.cleanup);
  cpSync(join(REPO, ".github"), join(s.dir, ".github"), { recursive: true });
  const toolchain = readFileSync(join(REPO, "rust-toolchain.toml"), "utf8");
  expect(toolchain).toContain('    "aarch64-apple-darwin",\n');
  writeFileSync(join(s.dir, "rust-toolchain.toml"), toolchain.replace('    "aarch64-apple-darwin",\n', ""));
  cpSync(join(REPO, "oneharness.toml"), join(s.dir, "oneharness.toml"));
  expect(checkContract(s.dir).join("\n")).toContain("differ from release.yml's build matrix");
});

test("a workflow whose jobs are not a mapping is refused rather than skipped", () => {
  const errors = mutated("notignored.yml", "jobs:\n  suppressions:", "jobs: []\nnot_jobs:\n  suppressions:");
  expect(errors.join("\n")).toContain("notignored.yml has no jobs mapping");
});

test("a matrix of an unexpected shape is reported, not crashed on", () => {
  const errors = mutated("ci.yml", "        os: [ubuntu-latest, macos-latest, windows-latest]\n    defaults:\n      run:\n        shell: bash\n    steps:\n      # Full history", "        os: ubuntu-latest\n    defaults:\n      run:\n        shell: bash\n    steps:\n      # Full history");
  expect(errors.join("\n")).toContain("ci.yml:check has a name, needs or strategy.matrix (os / include) of an unexpected shape");
});

test("a pull_request types list that drops synchronize is caught", () => {
  const errors = mutated("pr-lint.yml", "types: [opened, edited, reopened, synchronize]", "types: [opened, edited]");
  expect(errors.join("\n")).toContain("pr-lint.yml does not run on every pull request");
});

test("the llmlint job installing a harness other than oneharness.toml's primary is caught", () => {
  const errors = mutated("ci.yml", "npm install -g @openai/codex", "npm install -g @anthropic-ai/claude-code");
  expect(errors.join("\n")).toContain("ci.yml:llmlint must install oneharness.toml's primary harness (codex: npm install -g @openai/codex)");
});
