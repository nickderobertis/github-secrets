# AGENTS — github-workflows (macOS routing and its proofs)

The Nx project `github-workflows` (tag `type:tooling`): the macOS routing
commands the workflows run, the offline proofs over the real workflow graph,
and the Mac scheduling budgets. Its locked npm dependencies (`yaml`,
`@actions/expressions`, `@onebudgetspec/sdk`) live in `.github/package.json`
and are installed by `just setup-workflows`; nothing in the product uses them.

- `macos-policy.mjs classify` is the one release-only authority, shared by
  ci.yml's `policy` job and `scripts/ci-gate-tier.mjs`. Release-only means a
  same-repository head under `release-please--branches--<release branch>` and
  a fork-point diff that only bumps the repository's own (path, not registry)
  package versions plus `CHANGELOG.md`, compared semantically. Author and
  title carry no authority. Anything unreadable, forked or mixed — and any
  unrecognised event name — requires the Mac lane; known non-PR events never
  do. Widen it only by semantic comparison, never by path or name lists.
  Because the tier router imports it and a `scripts -> github-workflows` edge
  would be a cycle, `test` also runs `scripts/tests/ci-gate-tier.test.mjs`
  (that file only); `tests/selection.test.mjs` pins the wiring via Nx.
- The Mac lane is `workflows/macos.yml`, called by ci.yml and by the
  default-off `macos-diagnostic.yml`. The Linux carriers `check (macos-latest)`
  and `build (macos-latest)` keep the protected names and run
  `macos-result.mjs`, which passes only when the lane passed or the classifier
  authorized its omission (and, for build, the check legs passed).
- `workflow-graph.mjs` resolves the actual workflows — triggers, matrices,
  `if:` via `@actions/expressions`, reusable calls — and refuses any shape it
  does not model rather than counting it as zero Mac work. When a workflow
  gains a feature it refuses, extend the model and its tests, don't special-case.
- `test` records `.telemetry/macos-policy.json` (uncached; keyed to a digest of
  the source tree); `budgets` reads only that file through `macos-budget.mjs`
  and the onebudgetspec CLI, so thresholds live solely in `budgets.yaml`.
  `budgets` is a gate-wide target name: `just check` fans out over it with
  the others, so any project may register budgets under it.
