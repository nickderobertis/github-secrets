# AGENTS — gh-secrets-live-bitwarden (the live Bitwarden suite)

The Nx project `gh-secrets-live-bitwarden` (tag `type:live`):
`tests/e2e_live_bitwarden.rs` against a real, isolated Bitwarden account
through the real `bw` CLI.

- In `just check` its `test` target compiles it and runs every test as a
  runtime no-op (each logs `skip:` unless `GH_SECRETS_LIVE_TEST=1` and the
  `GH_SECRETS_BW_E2E_*` credentials are set). Never `#[cfg]` it out.
- The real run is the `live` target (`just test-live-bitwarden`, serial `-j1`
  because every test logs in to the one account), which the
  `live-e2e-bitwarden` CI job runs. Locally: `scripts/bw-e2e-env.sh just
  test-live-bitwarden`, which reads the isolated account's api-key credentials
  from the maintainer's own vault. Do not run it from an agent session.
- The offline twin of this suite is `tests/e2e/tests/e2e_bitwarden.rs` (a
  stand-in `bw`); keep the two covering the same contract from both sides.

<!-- llmlint: ignore-block[agents_md_durable_and_terse] moved here from the root AGENTS.md as this project's folder-scoped material, as the baseline task directs; condensing the catalogue is the separately scheduled AGENTS.md trim, not part of this move. -->
- `tests/e2e_live_bitwarden.rs` is the source
  half's real-API complement: it drives `sync`/`source list` against a real,
  *isolated* Bitwarden account (one that exists only for this test, so seeding
  and deleting items in it is safe). It proves the auth chain the wiremock
  suites can't — api-key login + master-password unlock + vault sync — then
  pulls real values off real items through every field selector (`password`,
  `#username`, `#notes`, `#fields.<NAME>`) to an env-file destination, asserts
  a no-op resync, proves `--default-field` changes what an unselected secret
  extracts (with a per-secret `#field` still overriding it), runs the full
  cold path through gh-secrets itself (login → unlock → sync → fetch), and
  confirms a wrong master password yields a precise unlock error with nothing
  written. Each test seeds uniquely-prefixed items
  via `bw` directly (the product CLI is write-only-blocked for Bitwarden) and
  deletes them in `Drop`. The hard-won isolation detail: every test points
  `BITWARDENCLI_APPDATA_DIR` at its own tempdir, and gh-secrets' spawned `bw`
  inherits it — so the suite never disturbs (or is confused by) a developer's
  real Bitwarden login in the default app-data location. See
  `tests/live_bw_common/mod.rs` beside it.
<!-- llmlint: ignore-end[agents_md_durable_and_terse] -->
