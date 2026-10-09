# AGENTS — gh-secrets-live-github (the live GitHub suite)

The Nx project `gh-secrets-live-github` (tag `type:live`): `tests/e2e_live.rs`
against the **real** GitHub API, plus `scripts/install.sh` against the real
latest release. Live contact puts it outside the deterministic gate's real runs:

- In `just check` its `test` target compiles it and runs every live test as a
  runtime no-op (each logs `skip:` unless `GH_SECRETS_LIVE_TEST=1`), so the live
  code cannot rot. Never `#[cfg]` it out.
- The real run is the `live` target (`just test-live`), which the `live-e2e`
  CI job runs with the `GH_E2E_TOKEN` and `GH_SECRETS_E2E_SANDBOX_REPO` Actions
  secrets. It needs `GH_TOKEN` with `repo` scope and
  `GH_SECRETS_E2E_SANDBOX_REPO` = the private sandbox repo's `owner/name`; the
  identity is configuration and never a literal in the tree. Unset or
  unreachable, the sandbox tests fail naming the key — they never skip. Do not
  run it from an agent session.
- Sandbox journeys live once, in `live_common/journeys.rs`; any offline proof
  (`tests/sandbox_fixture.rs`, against a loopback double) must call those same
  functions rather than restate them, so it proves what the live run does.
  `GH_SECRETS_LIVE_API_BASE` is a test-only, loopback-only override.
- `live_install_script_downloads_and_verifies_release` runs `scripts/install.sh`
  against the real release, which is what catches release-asset naming drift
  for users; the `install (<os>)` CI job proves the same path offline per PR.

<!-- llmlint: ignore-block[agents_md_durable_and_terse] this is the suite's coverage map — the one place a reader learns what is and is not proven here without reading every test — so it is kept whole beside the suite it describes. -->
- `tests/e2e_live.rs` round-trips the same `sync`
  pipeline (env-file source → real `github:` destination) against the real
  GitHub API: a secret becomes visible via the API after sync, a resync is a
  no-op, an updated source value advances `updated_at`, an invalid token
  surfaces a 401 the user can act on, and undeclaring a secret does not delete
  it remotely. The sandbox repo is shared across tests; isolation comes from a
  per-test secret-name prefix and a `Drop` cleanup.
<!-- llmlint: ignore-end[agents_md_durable_and_terse] -->
