# AGENTS — scripts (repository tooling)

The Nx project `scripts` (tag `type:tooling`): the toolchain and gate plumbing
(`bun.sh`, `nx`, `nx-base.sh`, `coverage.sh`, `msrv.sh`), CI's tier router
(`ci-gate-tier.mjs`), the installers (`install.sh`, `install-nextest.sh`,
`setup-llmlint.sh`, `session-setup.sh`) and the bench drivers, with their tests
in `tests/`. It depends on no other project and may depend only on
`type:tooling`; the live and bench projects depend on it (`install.sh`,
`bw-e2e-env.sh`, `bench*.sh`), so a change here reaches them without
reselecting the crate.

- Scripts are quiet on success and, on failure, print the problem and a
  concrete next action to stderr, then exit non-zero — except the two
  session-startup installers, which always exit 0 by design and log instead.
- Every script is tested as a real subprocess in a scratch directory, with only
  the external tool at the far end replaced (`bun`'s release served from
  `file://` via `GH_SECRETS_BUN_DOWNLOAD_BASE`, a fake `uv`, a recorder for Nx).
  `coverage-pipeline.test.mjs` builds a scratch crate under cargo-llvm-cov, so
  it is the slow one; it is why this is a project of its own.
- `install.sh` is the user-facing installer behind the README one-liner: keep it
  POSIX `sh`, and keep its asset names in lockstep with `release.yml` (the
  `install (<os>)` CI job and the live suite both prove that).
- `lint` syntax-checks every script with the shell that runs it; `test` runs
  `bun test scripts/tests` (the bash-script tests skip on Windows).
