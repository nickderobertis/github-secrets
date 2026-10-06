// Enforce the project graph's module boundaries (tools/project-boundaries.json).
//
// Nx's own enforce-module-boundaries rule is an ESLint rule over JS imports; the
// edges that matter here are Cargo dependencies, which Nx does not see. So this
// reads both layers directly:
//   * every Cargo workspace member (`cargo metadata --no-deps`) must have a
//     project.json beside its Cargo.toml, and each of its path dependencies on
//     another member (normal, dev or build) is an edge;
//   * every project.json's implicitDependencies are edges too.
// Each project must carry exactly one `type:*` tag, and every edge must be
// allowed by the constraint for the source project's tag.
//
// Usage: bun tools/check-project-boundaries.mjs [--root <workspace dir>]
// Exits 0 quietly when every edge is allowed; prints each violation and exits 1
// otherwise.
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";

// Project dirs relative to the root, always with `/` (git ls-files spells them
// that way on every platform; path.relative uses `\\` on Windows).
const rel = (from, to) => relative(from, to).split("\\").join("/") || ".";

const argv = process.argv.slice(2);
const rootFlag = argv.indexOf("--root");
const root = resolve(rootFlag >= 0 ? argv[rootFlag + 1] : join(import.meta.dir, ".."));

function fail(lines) {
  for (const line of lines) console.error(`project-boundaries: ${line}`);
  process.exit(1);
}

const rules = JSON.parse(readFileSync(join(root, "tools/project-boundaries.json"), "utf8"));
const constraints = new Map(rules.depConstraints.map((c) => [c.sourceTag, c.onlyDependOnTags]));

let metadata;
try {
  metadata = JSON.parse(
    execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps", "--offline"], {
      cwd: root,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    }),
  );
} catch (err) {
  fail([`'cargo metadata' failed: ${err.stderr || err.message}`, "fix the manifests so it resolves, then re-run."]);
}

// Projects, from every project.json git tracks (plus untracked ones in this tree).
const projectFiles = execFileSync(
  "git",
  ["ls-files", "--cached", "--others", "--exclude-standard", "--", "project.json", "**/project.json"],
  { cwd: root, encoding: "utf8" },
)
  .split("\n")
  .filter((f) => f && !f.startsWith("node_modules/"));

const projects = new Map(); // name -> { dir, tags, implicit }
const byDir = new Map(); // dir -> name
for (const file of projectFiles) {
  const json = JSON.parse(readFileSync(join(root, file), "utf8"));
  const dir = dirname(file);
  projects.set(json.name, { dir, tags: json.tags ?? [], implicit: json.implicitDependencies ?? [] });
  byDir.set(dir, json.name);
}

const errors = [];
const typeTag = (name) => {
  const tags = projects.get(name).tags.filter((t) => t.startsWith("type:"));
  if (tags.length !== 1) {
    errors.push(`${name} must carry exactly one type:* tag (has ${JSON.stringify(tags)}).`);
    return undefined;
  }
  if (!constraints.has(tags[0])) {
    errors.push(`${name}'s tag ${tags[0]} has no constraint in tools/project-boundaries.json.`);
    return undefined;
  }
  return tags[0];
};

// Cargo members -> projects, and their path-dependency edges.
const edges = []; // [from, to, how]
const memberDirs = new Map(); // manifest dir (relative) -> project name
for (const pkg of metadata.packages) {
  const dir = rel(root, dirname(pkg.manifest_path));
  const name = byDir.get(dir);
  if (!name) {
    errors.push(`Cargo member ${pkg.name} (${dir}/Cargo.toml) has no project.json beside it.`);
    continue;
  }
  memberDirs.set(dir, name);
}
for (const pkg of metadata.packages) {
  const from = memberDirs.get(rel(root, dirname(pkg.manifest_path)));
  if (!from) continue;
  for (const dep of pkg.dependencies) {
    if (!dep.path) continue;
    const to = memberDirs.get(rel(root, dep.path));
    if (to && to !== from) edges.push([from, to, `Cargo ${dep.kind ?? "normal"} dependency ${dep.name}`]);
  }
}
for (const [name, project] of projects) {
  for (const dep of project.implicit) {
    if (!projects.has(dep)) errors.push(`${name} names unknown implicit dependency ${dep}.`);
    else edges.push([name, dep, "implicitDependencies"]);
  }
}

for (const name of projects.keys()) typeTag(name);
for (const [from, to, how] of edges) {
  const fromTag = typeTag(from);
  const toTag = typeTag(to);
  if (!fromTag || !toTag) continue;
  const allowed = constraints.get(fromTag);
  if (!allowed.includes("*") && !allowed.includes(toTag)) {
    errors.push(
      `${from} (${fromTag}) may not depend on ${to} (${toTag}) — found ${how}. ` +
        `${fromTag} may depend only on ${allowed.join(", ")}.`,
    );
  }
}

if (errors.length) {
  fail([...new Set(errors), "see tools/project-boundaries.json for the allowed edges."]);
}
