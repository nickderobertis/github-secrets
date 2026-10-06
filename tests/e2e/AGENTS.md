# AGENTS — gh-secrets-e2e (the offline end-to-end suite)

The Nx project `gh-secrets-e2e` (tag `type:e2e`), a `publish = false` member
of the Cargo workspace. Everything here drives the **compiled** `gh-secrets`
binary as a subprocess (`assert_cmd`) — never the library, so the crate takes
no Cargo dependency on `gh-secrets`; its graph edge is the implicit dependency
in `project.json`, and its `test` target `dependsOn` `gh-secrets:build`.

- Offline and deterministic: GitHub is `wiremock` (via `GH_SECRETS_API_BASE`),
  state lives in tempdirs (`GH_SECRETS_HOME`), and the Bitwarden CLI is
  `support/fake_bw.rs` — a stand-in `bw` this crate builds and the Bitwarden
  journeys put first on `PATH`. That is a real subprocess double of an
  external tool, not a mock of the code under test: `gh-secrets` spawns it
  exactly as it spawns the real CLI. Never `#[ignore]` a test here.
- Under `just check` the suite runs instrumented (`scripts/coverage.sh test
  gh-secrets-e2e`): it drives the instrumented copy of the binary in
  `target/llvm-cov-target`, so the lines it reaches count toward the 95% floor
  over the `gh-secrets` crate. A behaviour you add to `src/` is usually covered
  best by a journey here.
- `tests/e2e_bitwarden.rs` is the offline twin of the live Bitwarden project:
  keep the two covering the same `bw` contract (login, unlock, sync, field
  selectors, scoping, failure edges) from both sides, and grow the fake `bw`
  only as far as a journey needs.
- `tests/common/mod.rs` holds the shared harness; the `mod.rs` form keeps cargo
  from building it as a test binary of its own.

<!-- llmlint: ignore-block[agents_md_durable_and_terse] moved here from the root AGENTS.md as this project's folder-scoped material, as the baseline task directs; condensing the catalogue is the separately scheduled AGENTS.md trim, not part of this move. -->
What each suite covers:

- The main wiremock suite (`tests/e2e.rs`) covers the unified surface: a
  pure-argument pipeline (`--from env:… --to github:… --secret …`) with a
  no-op re-sync and single-secret repush on change; `--to` replacing a
  config's destinations; `--only` filtering; `--secret NAME=ITEM` remapping
  through a real sync; the `--from github:` write-only rejection; the `store`
  group round-tripping through the encrypted vault (and asserting the file
  leaks neither names nor values); the local store as both source and
  destination; `check` reporting pending-then-clean without a GitHub token or
  a single PUT (config-driven and pure-args); explicit `--config` (with
  config-relative path/state resolution and a missing-path error) and
  `--state` overrides; global-config fallback, `--global`, and `list
  --global`; `init` / `--path` / `--global` incl. overwrite refusals; error
  edges (source missing a declared value, Bitwarden scoping flags on a
  non-bitwarden source, empty store name); and the structural sealed-box
  assertion on the PUT body so a broken seal step can't slip through. It also
  pins the failure/drift surface: the 401/403/404/500 message mapping on both
  the public-key GET and the PUT, the 204→"updated" report, malformed
  public-key rejection (wrong length, non-JSON); partial-failure semantics (a
  failed destination records *no* state, so the re-run repushes); the state
  file holding hashes only, and a deleted state file merely forcing a
  re-push; out-of-band edits to readable destinations (env file, local
  store) healed by `sync` while `check` stays state-only by design; hostile
  values (quotes, `$`, backticks, newlines) round-tripping env-destination →
  env-source with byte-identical canonical lines; invalid `--from`/`--to`/
  `--secret` specs and malformed/unknown-type configs erroring with the
  spec/file named; and `list` rendering the Bitwarden mapping (incl.
  `default_field` and per-secret `field` overrides) with no credentials.
- The config-driven e2e suite (`tests/e2e_manifest.rs`) drives the binary
  through `init`, `list`, and `sync` against a checked-in `gh-secrets.json`:
  pushes to GitHub (wiremock) and a `.env` destination simultaneously;
  verifies the PUT body is sealed-box shaped and the plaintext never appears
  in it; verifies a re-sync of unchanged values produces zero new PUTs and
  zero env-file writes; verifies a source-side value change repushes only the
  affected secret.
- The auth e2e suite (`tests/e2e_auth.rs`) drives the `gh-secrets auth` command
  group and proves the credential precedence end-to-end: `auth status` reports
  provenance without printing values; storing/clearing round-trips through the
  encrypted vault (the file is `0600`, contains no plaintext token, fails fast
  without a passphrase in a non-interactive run, and rejects a wrong
  passphrase with a decryption error); the session lifecycle (`auth unlock`
  lets fresh processes read *and* write with no passphrase anywhere in the
  environment, expiry and `auth lock` revoke it, a session cannot extend
  itself, and a session left over from a *recreated* vault is detected as
  mismatched and deleted on sight); selective clears (`--github` /
  `--bitwarden`) and the empty-token / no-flag / lock-with-no-session edges;
  and — the key assertion — a real `sync`
  against a wiremock GitHub records the exact `Authorization` bearer, so each
  test can confirm the token that *won* (shell env, `.env`, `.env.local`, or
  stored config) is the one that actually reached the API. The dotenv
  parser/precedence planner is also unit-tested in `src/envfile.rs`.
<!-- llmlint: ignore-end[agents_md_durable_and_terse] -->
