// Facts that have one source but are restated elsewhere for readers, reconciled
// so a restatement cannot drift from what is enforced:
//
//   * the line-coverage floor — source: MIN_LINES in scripts/coverage/coverage.sh (what
//     `coverage-aggregate:coverage` enforces); restated as `NN%` on lines about lines,
//     coverage or the floor in the AGENTS.md files, the justfile and project.json
//     files;
//   * the MSRV — source: `rust-version` in Cargo.toml's [workspace.package];
//     restated as clippy.toml's `msrv` (so clippy flags too-new APIs at the same
//     floor) and as "MSRV is X.Y" in the AGENTS.md files;
//   * the release version — source: the root package's `version`, which
//     release-please bumps; restated by every workspace member's `version` and
//     by .release-please-manifest.json.
//
// Usage: bun tools/check-restated-facts.mjs [--root <dir>]
// Exit status: 0 (quiet) when every restatement agrees; 1 with each drift
// printed; 2 on a usage error.
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, statSync } from "node:fs";
import { join, resolve } from "node:path";

function tracked(root, patterns) {
  return execFileSync("git", ["ls-files", "--cached", "--others", "--exclude-standard", "--", ...patterns], {
    cwd: root,
    encoding: "utf8",
  })
    .split("\n")
    .filter((f) => f && !f.startsWith("node_modules/"));
}

function eachLine(root, files, visit) {
  for (const file of files) {
    readFileSync(join(root, file), "utf8")
      .split("\n")
      .forEach((line, i) => visit(file, i + 1, line));
  }
}

export function checkCoverageFloor(root) {
  const source = readFileSync(join(root, "scripts/coverage/coverage.sh"), "utf8").match(/^readonly MIN_LINES=(\d+)$/m);
  if (!source) return ["scripts/coverage/coverage.sh no longer declares `readonly MIN_LINES=<n>`; restore it (it is the floor's one source)."];
  const floor = source[1];
  const errors = [];
  eachLine(root, tracked(root, ["*AGENTS.md", "justfile", "*project.json"]), (file, n, line) => {
    if (!/\b(lines?|coverage|floor)\b/i.test(line)) return;
    for (const m of line.matchAll(/\b(\d{1,3})%/g)) {
      if (m[1] !== floor) errors.push(`${file}:${n} states a ${m[1]}% coverage floor; scripts/coverage/coverage.sh enforces ${floor}%.`);
    }
  });
  return errors;
}

export function checkMsrv(root) {
  let msrv;
  try {
    msrv = Bun.TOML.parse(readFileSync(join(root, "Cargo.toml"), "utf8")).workspace?.package?.["rust-version"];
  } catch (err) {
    return [`Cargo.toml is not readable TOML: ${err.message}`];
  }
  if (typeof msrv !== "string") return ["Cargo.toml must declare [workspace.package] rust-version (the MSRV's one source)."];
  const errors = [];
  let clippy;
  try {
    clippy = Bun.TOML.parse(readFileSync(join(root, "clippy.toml"), "utf8")).msrv;
  } catch (err) {
    errors.push(`clippy.toml is not readable TOML: ${err.message}`);
  }
  if (clippy !== msrv) errors.push(`clippy.toml msrv ${JSON.stringify(clippy)} differs from Cargo.toml rust-version "${msrv}"; make them equal.`);
  eachLine(root, tracked(root, ["*AGENTS.md"]), (file, n, line) => {
    for (const m of line.matchAll(/\bMSRV is (\d+\.\d+(?:\.\d+)?)/g)) {
      if (m[1] !== msrv) errors.push(`${file}:${n} states MSRV ${m[1]}; Cargo.toml declares ${msrv}.`);
    }
  });
  return errors;
}

export function checkVersions(root) {
  const toml = (file) => Bun.TOML.parse(readFileSync(join(root, file), "utf8"));
  let manifest;
  try {
    manifest = toml("Cargo.toml");
  } catch (err) {
    return [`Cargo.toml is not readable TOML: ${err.message}`];
  }
  const version = manifest.package?.version;
  if (typeof version !== "string") return ["Cargo.toml's root package must declare a literal `version` (release-please bumps it)."];
  const errors = [];
  const members = manifest.workspace?.members ?? [];
  if (!Array.isArray(members) || !members.every((m) => typeof m === "string" && m.length > 0)) {
    return ["Cargo.toml [workspace] members must be a list of member paths."];
  }
  for (const member of members) {
    let v;
    try {
      v = toml(`${member}/Cargo.toml`).package?.version;
    } catch (err) {
      errors.push(`${member}/Cargo.toml is not readable TOML: ${err.message}`);
      continue;
    }
    if (v !== version) errors.push(`${member}/Cargo.toml version ${JSON.stringify(v)} differs from the root package's "${version}"; release-please keeps them in lockstep.`);
  }
  try {
    const released = JSON.parse(readFileSync(join(root, ".release-please-manifest.json"), "utf8"))["."];
    if (released !== version) errors.push(`.release-please-manifest.json records ${JSON.stringify(released)} but Cargo.toml says "${version}"; never hand-bump either.`);
  } catch (err) {
    errors.push(`.release-please-manifest.json is not readable JSON: ${err.message}`);
  }
  return errors;
}

if (import.meta.main) {
  const argv = process.argv.slice(2);
  let root = resolve(join(import.meta.dir, ".."));
  if (argv.length) {
    if (argv.length !== 2 || argv[0] !== "--root" || !existsSync(argv[1]) || !statSync(argv[1]).isDirectory()) {
      console.error(`usage: bun ${process.argv[1]} [--root <existing directory>] (got: ${argv.join(" ")})`);
      process.exit(2);
    }
    root = resolve(argv[1]);
  }
  const errors = [...checkCoverageFloor(root), ...checkMsrv(root), ...checkVersions(root)];
  if (errors.length) {
    for (const e of errors) console.error(`restated-facts: ${e}`);
    process.exit(1);
  }
}
