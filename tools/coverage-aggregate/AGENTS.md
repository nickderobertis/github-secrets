# AGENTS — coverage-aggregate (the repo-level coverage gate)

`coverage-clear` runs before the crates' instrumented test runs and `coverage`
merges their profiles and holds the gh-secrets crate's `src/` to the floor in
`scripts/coverage/coverage.sh`. A new crate whose tests should count carries the
`coverage:profiles` tag; that tag is how both the dependency edge and the
`coverage` prerequisites find it, so there is no list here to update.
