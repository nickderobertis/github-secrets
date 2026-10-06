# AGENTS — workspace (repo-level targets)

The Nx project `workspace` (tag `type:workspace`) holds the targets that belong
to the repository rather than one crate. It implicitly depends on every other
project, so any change selects it — that is what lets the coverage aggregate
always see a complete set of profiles.

- `lint` runs the repo's reconciling checks: `check-project-boundaries.mjs`
  (the tag rule in `project-boundaries.json`, over Cargo path dependencies
  *and* Nx implicit dependencies — Nx's own boundary rule only sees JS
  imports), `check-workflow-contract.mjs` (the fixed CI contexts and what may
  gate them, plus the CI facts restated across workflows) and
  `check-coverage-floor.mjs` (every restated floor equals `MIN_LINES`). When
  you add a project, give it exactly one `type:*` tag; when you rename a CI job
  or add a condition to one that reports a fixed context, these tell you.
- `test` runs `bun test tools/tests`: the repo's own tooling driven as real
  subprocesses in scratch repositories, with only the external tool at the far
  end (Nx, bun's release, uv, git remotes) replaced. Keep new tooling covered
  the same way; the bash-script tests skip on Windows.
- `coverage-clear` / `coverage` are steps one and three of
  `scripts/coverage.sh` (each crate's `test` target is step two). `coverage`
  depends on `^test` — the `test` of every project, since `implicitDependencies`
  is `"*"` — so a new crate joins the union with no list to edit, and it
  enforces 95% lines over the gh-secrets crate's `src/`. Lower the floor only with a recorded reason in the root
  AGENTS.md; never exclude product code to meet it.
- `supply-chain` (`just supply-chain`) runs cargo-deny against `deny.toml` and
  cargo-machete; Linux-only, in its own CI job, not part of `check`.
- Everything here runs on the bun pinned in `.tool-versions` (scripts/nx
  puts it first on PATH) and uses Node/bun built-ins only — no npm dependencies.
