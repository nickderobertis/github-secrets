// The line-coverage floor has one source — MIN_LINES in scripts/coverage.sh,
// which is what `workspace:coverage` enforces — and is restated for readers in
// the AGENTS.md files, the justfile and the project definitions. This check
// fails when a restatement (a `NN%` on a line that talks about lines, coverage
// or the floor) disagrees with the source, so the docs cannot drift from the gate.
//
// Usage: bun tools/check-coverage-floor.mjs [--root <dir>]
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, statSync } from "node:fs";
import { join, resolve } from "node:path";

export function checkCoverageFloor(root) {
  const source = readFileSync(join(root, "scripts/coverage.sh"), "utf8").match(/^readonly MIN_LINES=(\d+)$/m);
  if (!source) return ["scripts/coverage.sh no longer declares `readonly MIN_LINES=<n>`; restore it (it is the floor's one source)."];
  const floor = source[1];
  const files = execFileSync("git", ["ls-files", "--cached", "--others", "--exclude-standard", "--", "*AGENTS.md", "justfile", "*project.json"], {
    cwd: root,
    encoding: "utf8",
  })
    .split("\n")
    .filter((f) => f && !f.startsWith("node_modules/"));
  const errors = [];
  for (const file of files) {
    readFileSync(join(root, file), "utf8")
      .split("\n")
      .forEach((line, i) => {
        if (!/\b(lines?|coverage|floor)\b/i.test(line)) return;
        for (const m of line.matchAll(/\b(\d{1,3})%/g)) {
          if (m[1] !== floor) errors.push(`${file}:${i + 1} states a ${m[1]}% coverage floor; scripts/coverage.sh enforces ${floor}%.`);
        }
      });
  }
  return errors;
}

if (import.meta.main) {
  const argv = process.argv.slice(2);
  let root = resolve(join(import.meta.dir, ".."));
  if (argv.length) {
    if (argv.length !== 2 || argv[0] !== "--root" || !existsSync(argv[1]) || !statSync(argv[1]).isDirectory()) {
      console.error(`usage: bun ${process.argv[1]} [--root <existing directory>]`);
      process.exit(2);
    }
    root = resolve(argv[1]);
  }
  const errors = checkCoverageFloor(root);
  if (errors.length) {
    for (const e of errors) console.error(`coverage-floor: ${e}`);
    process.exit(1);
  }
}
