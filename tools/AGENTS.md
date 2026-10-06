# AGENTS — workspace (repo-level targets)

The Nx project `workspace` (tag `type:workspace`) holds the targets that belong
to the repository rather than one crate. It implicitly depends on every other
project, so any change selects it — that is what lets the coverage aggregate
always see a complete set of profiles.

- `lint` runs `check-project-boundaries.mjs` (the tag rule in
  `project-boundaries.json`, over Cargo path dependencies *and* Nx implicit
  dependencies — Nx's own boundary rule only sees JS imports) and
  `check-workflow-contract.mjs` (the fixed status-check contexts and what may
  gate them, `contents: read` in ci.yml, pinned `just` in every ci.yml job,
  and packaging lockstep between CI's install job and release.yml). When you
  add a project, give it exactly one `type:*` tag and, if it is a new kind, a
  constraint; when you rename a CI job or add a condition to one that reports a
  fixed context, this check is what will tell you.
- `test` runs `bun test tools/tests`: real subprocess tests of the repo's own
  tooling — the boundary checker on a scratch Cargo workspace, the gate's tier
  selection (`scripts/nx-tier.sh`), CI's tier routing
  (`scripts/ci-gate-tier.mjs`) fed synthetic event payloads, the pre-push hook
  through real `git push`es, `scripts/install.sh --from-dir`, and the workflow
  contract against mutated copies. The bash-script tests skip on Windows.
- `coverage-clear` / `coverage` are steps one and three of
  `scripts/coverage.sh` (each crate's `test` target is step two). `coverage`
  depends on every crate's `test` and enforces 95% lines over the gh-secrets
  crate's `src/`. Lower the floor only with a recorded reason in the root
  AGENTS.md; never exclude product code to meet it.
- `supply-chain` (`just supply-chain`) runs cargo-deny against `deny.toml` and
  cargo-machete; Linux-only, in its own CI job, not part of `check`.
- Everything here runs on the bun pinned in `.tool-versions` (scripts/nx.sh
  puts it first on PATH) and uses Node/bun built-ins only — no npm dependencies.
