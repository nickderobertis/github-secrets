// Restated facts (the coverage floor, the MSRV) are reconciled against their one source.
import { afterEach, expect, test } from "bun:test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { checkCoverageFloor, checkMsrv } from "../check-restated-facts.mjs";
import { REPO, gitRepo, scratch } from "../../scripts/tests/helpers.mjs";

let cleanups = [];
afterEach(() => {
  for (const c of cleanups) c();
  cleanups = [];
});

function repoWith(agents, floor = 95) {
  const s = scratch();
  cleanups.push(s.cleanup);
  mkdirSync(join(s.dir, "scripts"));
  writeFileSync(join(s.dir, "scripts/coverage.sh"), `#!/usr/bin/env bash\nreadonly MIN_LINES=${floor}\n`);
  writeFileSync(join(s.dir, "AGENTS.md"), agents);
  gitRepo(s.dir);
  return s.dir;
}

test("the committed restatements agree with the enforced floor", () => {
  expect(checkCoverageFloor(REPO)).toEqual([]);
});

test("a doc stating a different floor is caught, naming file and line", () => {
  const errors = checkCoverageFloor(repoWith("# x\n\nCoverage: 90% lines over src/.\nUnrelated 50% of nothing.\n"));
  expect(errors).toEqual(["AGENTS.md:3 states a 90% coverage floor; scripts/coverage.sh enforces 95%."]);
});

test("a coverage.sh without MIN_LINES is refused", () => {
  const dir = repoWith("ok\n");
  writeFileSync(join(dir, "scripts/coverage.sh"), "#!/usr/bin/env bash\n");
  expect(checkCoverageFloor(dir)[0]).toContain("no longer declares");
});

function msrvRepo({ manifest = "1.86", clippy = "1.86", agents = "MSRV is 1.86.\n" } = {}) {
  const s = scratch();
  cleanups.push(s.cleanup);
  writeFileSync(join(s.dir, "Cargo.toml"), `[workspace]\nmembers = []\n\n[workspace.package]\nrust-version = "${manifest}"\n`);
  writeFileSync(join(s.dir, "clippy.toml"), `msrv = "${clippy}"\n`);
  writeFileSync(join(s.dir, "AGENTS.md"), agents);
  gitRepo(s.dir);
  return s.dir;
}

test("the committed MSRV restatements agree with Cargo.toml", () => {
  expect(checkMsrv(REPO)).toEqual([]);
});

test("a clippy msrv or doc that drifts from rust-version is caught", () => {
  expect(checkMsrv(msrvRepo({ clippy: "1.85" }))).toEqual(['clippy.toml msrv "1.85" differs from Cargo.toml rust-version "1.86"; make them equal.']);
  expect(checkMsrv(msrvRepo({ agents: "the MSRV is 1.80 here\n" }))).toEqual(["AGENTS.md:1 states MSRV 1.80; Cargo.toml declares 1.86."]);
});
