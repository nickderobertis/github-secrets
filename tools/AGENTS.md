# AGENTS — workspace (repo-level targets)

The Nx project `workspace` (tag `type:workspace`) holds the targets that belong
to the repository rather than one crate. It implicitly depends on every other
project, so any change selects it — that is what lets the coverage aggregate
always see a complete set of profiles.

- `lint` runs the reconciling checks. Nx's own boundary rule only sees JS
  imports, so `check-project-boundaries.mjs` reads Cargo path dependencies and
  Nx implicit dependencies itself: give every new project exactly one `type:*`
  tag. When a CI job reporting a fixed context is renamed or gains a
  condition, `check-workflow-contract.mjs` is what fails.
- `coverage` depends on the `test` of every project tagged `coverage:profiles`;
  a new crate whose tests should count toward the floor carries that tag.
- `supply-chain` (`just supply-chain`) runs cargo-deny against `deny.toml` and
  cargo-machete; Linux-only, in its own CI job, not part of `check`.
- Everything here runs on the bun pinned in `.tool-versions` (scripts/nx
  puts it first on PATH) and uses Node/bun built-ins only — no npm dependencies.
