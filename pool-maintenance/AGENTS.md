# AGENTS — pool-maintenance (the maintenance pass-time budget)

The Nx project `pool-maintenance` (tag `type:tooling`) holds the one budget on
the pool maintenance this repository declares for onevcs (`onevcs.toml` runs
`just pool-maintenance`, the pinned `cargo-prune-superseded` over `target`):
`budgets.yaml`, `measure.sh`, which exists only for that budget, and
`measure.test.mjs`, its end-to-end test on a scratch workspace.

- The budget builds the workspace twice, so it sits only in the standard
  `budgets-host` target, which neither `just check` nor `just budgets` selects
  (both run the gate-wide `budgets` target alone). Only the release
  PR's `pool-maintenance-budget` CI job runs it (`just pool-maintenance-budget`),
  after the gate. `lint` and `test` are cheap and run with every change that
  reaches this project.
- `measure.sh` times a steady-state pass, never a first pass over history: an
  untimed pass clears the backlog, an alternative-profile gate build backdated
  past the window stands in for one run's superseded builds, the ordinary gate
  build follows, and only the last pass is the figure (the pruner's own
  onebudgetspec report; the backlog pass is appended to its detail). It fails
  unless that pass removed only the alternative build's entries. Linux only.
- The recipes it drives (`pool-maintenance`, `install-prune-superseded`) live
  in the root justfile and are tested in `tools/tests/pool-maintenance.test.mjs`,
  which a justfile change reaches; this project's inputs are its own files.
