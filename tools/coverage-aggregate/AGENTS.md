# AGENTS — coverage-aggregate (the repo-level coverage gate)

`coverage-clear` runs before the crates' instrumented test runs and `coverage`
merges their profiles and holds the gh-secrets crate's `src/` to the floor in
`scripts/coverage/coverage.sh`. A new crate whose tests should count carries the
`coverage:profiles` tag; that tag is how both the dependency edge and the
`coverage` prerequisites find it, so there is no list here to update.

The report also waits on the `type:live` crates' `test`: they run instrumented
into the same directory (as runtime no-ops), and `llvm-cov report` reads every
object there, so reporting while one links fails on a half-written binary. That
edge only orders tasks; it adds no implicit dependency, so a live-crate change
alone still does not select the coverage gate.
