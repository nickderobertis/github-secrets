# AGENTS — coverage-aggregate (the repo-level coverage gate)

`coverage-clear` runs before the crates' instrumented test runs and `coverage`
merges their profiles and holds the gh-secrets crate's `src/` to the floor in
`scripts/coverage/coverage.sh`. A new crate whose tests should count carries the
`coverage:profiles` tag; that tag is how both the dependency edge and the
`coverage` prerequisites find it, so there is no list here to update.

The report reads every object in `target/llvm-cov-target`, so only the counted
crates may build there. The `type:live` crates' gate `test` runs uninstrumented
into `target/debug` (it is a runtime no-op whose lines are not counted), which
keeps a live binary from being linked under a running report without an edge
from this gate to the live projects.
