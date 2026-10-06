// The workflow contract checker against the real workflows, and against copies
// of them with one realistic regression each.
import { afterEach, expect, test } from "bun:test";
import { cpSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { checkContract } from "../check-workflow-contract.mjs";
import { REPO, scratch } from "./helpers.mjs";

let cleanups = [];
afterEach(() => {
  for (const c of cleanups) c();
  cleanups = [];
});

function mutated(file, from, to) {
  const s = scratch();
  cleanups.push(s.cleanup);
  cpSync(join(REPO, ".github"), join(s.dir, ".github"), { recursive: true });
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
