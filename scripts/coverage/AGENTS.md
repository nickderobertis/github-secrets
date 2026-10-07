# AGENTS — coverage (the line-coverage driver)

The Nx project `coverage` (tag `type:tooling`): `coverage.sh` and its
end-to-end test. Each crate's `test` target runs `coverage.sh test <crate>`
(instrumented, `--no-report`), `coverage-aggregate:coverage-clear` runs `clear`, and
`coverage-aggregate:coverage` runs `report` — the floor is enforced once over the union
of every crate's profiles, so the e2e journeys count toward the crate they drive.

- `MIN_LINES` is the floor's one source; `tools/check-restated-facts.mjs` holds
  every restatement to it. Lower it only with a reason recorded in the root
  AGENTS.md, and never exclude product code (`src/`) to meet it — the report
  leaves out only the test and bench members under `tests/` and `benches/`.
- Windows runs the tests uninstrumented and skips the report: the spawned
  binary's coverage is not attributed there.
- The test is self-contained (no imports from the `scripts` project) so this
  project has no dependency edge and only a driver edit reruns it.
