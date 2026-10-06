# AGENTS — workspace (repo-level targets)

The Nx project `workspace` (tag `type:workspace`) holds the checks that belong
to the repository rather than one crate. It implicitly depends on every other
project, so any change re-runs them; they are cheap and uncached. The coverage
gate is the separate `coverage-aggregate` project (`tools/coverage-aggregate/`).

- `lint` runs the reconciling checks. Nx's own boundary rule only sees JS
  imports, so `check-project-boundaries.mjs` reads Cargo path dependencies and
  Nx implicit dependencies itself: give every new project exactly one `type:*`
  tag. When a CI job reporting a fixed context is renamed or gains a
  condition, `check-workflow-contract.mjs` is what fails.
- `supply-chain` (`just supply-chain`) runs cargo-deny against `deny.toml` and
  cargo-machete; Linux-only, in its own CI job, not part of `check`.
- Everything here runs on the bun pinned in `.tool-versions` (scripts/nx
  puts it first on PATH) and uses Node/bun built-ins only — no npm dependencies.
