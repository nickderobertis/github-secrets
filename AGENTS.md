# AGENTS.md

Durable instructions for humans and agents working in this repo. Write for a
future maintainer, not as a session log. Put deterministic steps in scripts and
keep this file for constraints, tradeoffs, and judgment.

> `CLAUDE.md` is a symlink to this file (`ln -s AGENTS.md CLAUDE.md`) so the two
> never drift. Edit `AGENTS.md` only.

## What this repo is

`gh-secrets` is a single-binary Rust CLI for managing GitHub Actions repository
secrets in bulk. It has two distinct workflows:

1. **Profile-based** (`gh-secrets token | repo | secrets | record | check`):
   the user keeps a local config of profiles, included/excluded repositories,
   and global + per-repository secret values; `gh-secrets secrets sync` then
   pushes only the secrets that have changed since the last sync.
   `gh-secrets secrets list [repo]` reports stored secret names (global +
   per-repo), never values.

2. **Manifest-based** (`gh-secrets manifest init|list|sync`,
   `gh-secrets source list`): a repo-local `gh-secrets.json` declares an
   external `source` (today: Bitwarden) and one or more `destinations` (today:
   GitHub Actions secrets, dotenv file). `gh-secrets manifest sync` pulls every
   managed secret from the source and pushes to each destination that doesn't
   already hold the current value. Idempotent across runs via a co-located
   `.gh-secrets-state.json` (gitignore it) that stores per-(secret,
   destination) SHA-256 hashes — the plaintext value is never persisted there.
   Each managed secret has a *source-side* identity (`name`, plus an optional
   `item`/`field` to look up a differently-named source entry) and a
   *destination-side* identity (`destination_names`). When `destination_names`
   is omitted it defaults to `[name]` — the common "same name everywhere" case
   needs no extra config. Supplying it lets the destination name differ from the
   source identity and lets one source value fan out to several destination
   names (e.g. a single publish token pushed as both `NPM_TOKEN` and
   `NODE_AUTH_TOKEN`); the value is fetched once and pushed under each name, and
   each (destination-name, destination) pair tracks its own hash in the state
   file. Destination names must be unique across the whole manifest —
   `RepoManifest::load` rejects a config where two managed secrets resolve to the
   same destination name (which would otherwise race to last-writer-wins), so the
   error surfaces at load time before any source contact. `gh-secrets manifest list` reports the secrets the manifest *declares*
   (each name plus its resolved source item/field, and the fan-out arrow `->
   NAME, NAME` when `destination_names` is set), reading only the checked-in
   file.
   `gh-secrets source list` instead *enumerates the source itself* — it unlocks
   the configured source (e.g. the Bitwarden vault, scoped by the manifest's
   collection/organization) and prints every available item's name and id, so a
   user can discover which item names exist to wire into the manifest. Both
   honor the never-print-a-value invariant: `manifest list` reads only mapping
   metadata, and `source list` requests item *identity* only, never field
   values.

## Command surface

Use the `just` recipes; do not hand-roll equivalent commands.

- `just bootstrap` — fetch the toolchain components and `cargo fetch`.
- `just check` — full quality gate: `cargo fmt --check`, `cargo clippy -D
  warnings`, `cargo nextest run` (unit + integration), and `test-e2e` (the
  wiremock-driven e2e suite plus a compile-and-skip pass of the live suite).
  Must pass before any commit or PR.
- `just test` / `just test-e2e` — fast unit/integration tests, or the e2e
  suite that drives the compiled binary against a mock GitHub server.
- `just test-live` — opt-in: run the live e2e suite against the real GitHub
  API. Requires `GH_TOKEN` with `repo` scope; creates (idempotently) a private
  sandbox repo `gh-secrets-e2e-sandbox` on the authenticated user's account
  and cleans up the secrets it creates.
- `just lint` / `just format` — clippy / rustfmt.
- `just upgrade` — `cargo update`, then re-run `just check`.

The product binary is `gh-secrets`. `cargo run -- <args>` invokes it during
development; the e2e tests invoke the compiled artifact via `assert_cmd` so
they exercise it the way a user does.

## Invariants (non-negotiable)

- The gate is strict: `clippy` runs with `-D warnings`, `rustfmt` is enforced
  in check mode, and there is no warnings-only mode. A diagnostic is either an
  error or has a tracked rationale.
- Validate all external input at trust boundaries: CLI arguments via clap, the
  on-disk config via serde + explicit field defaults, and GitHub API responses
  via typed structs that reject unknown variants of the small enums we care
  about (visibility, encryption key id, etc.).
- E2E is part of the default gate, not opt-in. The wiremock e2e suite
  (`tests/e2e.rs`) is plain `#[test]`-driven (no `#[ignore]`), spins up a
  mocked GitHub API with `wiremock`, and drives the compiled binary. Live
  GitHub credentials are never required to run the gate. The live e2e suite
  (`tests/e2e_live.rs`) exists alongside it for opt-in real-API coverage —
  each test runtime-skips with a logged `skip:` line when
  `GH_SECRETS_LIVE_TEST=1` is not set, so the default gate still compiles and
  exercises that code path as a no-op (catching breakage in the live test
  helpers without making any network call).
- The CLI never prints the value of a secret to stdout, stderr, log lines, or
  error messages. Secret values are also never written into the configured
  `GH_SECRETS_HOME` path other than in the encrypted-at-rest-by-the-user JSON
  config that the user explicitly opted into.
- Cross-platform: build and test on Linux, macOS, and Windows in CI.
- Do not commit secrets, credentials, PII, or customer data.

## Config and paths

The CLI reads these config files under a single root:

- `<root>/app.json` — global app config (current profile, profile list).
- `<root>/profiles/<name>.json` — per-profile state (GitHub token,
  include/exclude lists, secrets, sync records). Used by the profile flow.
- `<root>/credentials.json` — profile-independent credential store for the
  **manifest** flow (GitHub token + Bitwarden login), written by `gh-secrets
  auth`. `0600` on Unix; treat as sensitive. It is the lowest-priority
  credential layer (see the precedence below).

The root is resolved as `$GH_SECRETS_HOME` if set, otherwise the platform
config directory (`$XDG_CONFIG_HOME/gh-secrets` on Linux,
`~/Library/Application Support/gh-secrets` on macOS,
`%APPDATA%\gh-secrets` on Windows). Tests use `GH_SECRETS_HOME` pointed at a
tempdir so they never touch the user's real config.

The GitHub API base is `https://api.github.com` and is overridable for tests
via `GH_SECRETS_API_BASE`. That override exists *only* so the e2e suite can
point at `wiremock`; it is intentionally undocumented in `--help`.

For the manifest-driven flow:

- `gh-secrets.json` lives at the repo root (or wherever the user invokes
  `gh-secrets manifest sync` against). It is checked into source control.
- `.gh-secrets-state.json` sits next to it and stores per-(secret,
  destination) SHA-256 hashes that drive the "push only when changed" check.
  **Always gitignore this file** — losing it forces a re-push but leaks
  nothing.
- Credential resolution for the manifest flow (the GitHub token *and* the
  Bitwarden login) follows a single precedence: **shell env > `.env` >
  `.env.local` > stored config**. `manifest sync` and `auth status` auto-load
  `.env` then `.env.local` from the current directory into the process
  environment, setting only keys that aren't already present — so a real shell
  variable wins, then `.env`, then `.env.local`. The lowest layer is
  `<root>/credentials.json`, written by `gh-secrets auth github <token>` and
  `gh-secrets auth bitwarden --client-id/--client-secret/--master-password`;
  `gh-secrets auth status` reports where each credential resolves from without
  ever printing a value, and `gh-secrets auth clear [--github|--bitwarden]`
  removes it. Dotenv auto-load is deliberately scoped to these
  credential-consuming commands, not every invocation: the profile flow keeps
  its token in its own config and never reads these vars, and a global load
  would pull a developer's real `.env` into unrelated subprocesses (the test
  suites run with the repo root as CWD, where a real `.env` lives).
- The GitHub destination has no per-manifest token field by design — the
  manifest is checked in, the token is not. The token resolves via the
  precedence above (`GH_TOKEN` preferred, then `GITHUB_TOKEN`, then stored
  config).
- The Bitwarden source shells out to the `bw` (password-manager) CLI, which
  must be on `$PATH` (`npm install -g @bitwarden/cli` or
  `brew install bitwarden-cli`). In a fresh environment (CI) it expects
  `BW_CLIENTID`, `BW_CLIENTSECRET` (personal API key) and `BW_PASSWORD`
  (master password) so it can `bw login --apikey` and `bw unlock --raw
  --passwordenv BW_PASSWORD`. The personal API key only *authenticates* — the
  master password is still required to unlock the vault, so all three are
  needed for a fresh login. If `BW_SESSION` is already set (e.g. local dev
  where the user is already unlocked), it's used as-is and the other three are
  ignored. Each credential is also read from a `BITWARDEN_*` alias when the
  canonical `BW_*` name is unset: `BITWARDEN_CLIENT_ID`,
  `BITWARDEN_CLIENT_SECRET`, `BITWARDEN_MASTER_PASSWORD` (or
  `BITWARDEN_PASSWORD`), and `BITWARDEN_SESSION`. The canonical name wins when
  both are set; an empty value counts as unset. These vars follow the
  precedence above: `manifest sync` auto-loads `.env`/`.env.local`, and any
  field still unset then falls back to the `gh-secrets auth bitwarden` stored
  config. Whatever layer supplies a value, the `bw` subprocess always receives
  it under the canonical `BW_*` name. `BW_SESSION`/`BITWARDEN_SESSION` is the
  one credential never read from stored config — it's an ephemeral unlock
  token, not a durable credential.
- A second test-only override, `GH_SECRETS_TEST_SOURCE_FILE`, points the
  manifest's source resolver at a JSON file `{ "NAME": "value", ... }`
  instead of contacting Bitwarden. Used exclusively by `tests/e2e_manifest.rs`
  and intentionally undocumented in `--help`, mirroring `GH_SECRETS_API_BASE`.

## Scripts and output are context

- Scripts and the CLI itself are quiet on success — a single line, or nothing.
- On failure, print the exact error and a concrete suggested next action to
  stderr, and exit non-zero.
- Treat all command output as context the next agent has to read: maximize
  signal, minimize noise.

## Tests are context engineering

- Tests are how you and future agents actually see this system behave. Invest
  in them deliberately.
- The default coverage strategy is: a thin layer of unit tests for the secret
  bookkeeping in `secrets.rs` / `sync.rs`, and end-to-end tests in
  `tests/e2e*.rs` that invoke `gh-secrets` as a subprocess. When you touch a
  feature, prefer extending the e2e suite — it sees the same thing the user
  sees.
- The wiremock e2e suite covers the happy path (configure profile, add
  secrets, sync to a repo, observe a no-op re-sync), failure/recovery (auth
  error → `gh-secrets token <new>` → success), every subcommand on the local
  state surface, `record fill`/`reset` semantics, and a structural assertion
  on the PUT body shape so a broken seal step can't slip through.
- The live e2e suite (`tests/e2e_live.rs`) round-trips a small handful of
  flows against the real GitHub API: a secret becomes visible via the API
  after sync, a resync is a no-op, an updated value advances `updated_at`,
  an invalid token surfaces a 401 the user can act on, the per-repo override
  path runs end-to-end, and a local `secrets remove` does not delete the
  remote secret. The sandbox repo is shared across tests; isolation comes
  from a per-test secret-name prefix and a `Drop` cleanup.
- The manifest-flow e2e suite (`tests/e2e_manifest.rs`) drives the binary
  through `manifest init` and `manifest sync` end-to-end: pushes to GitHub
  (wiremock) and a `.env` destination simultaneously; verifies the PUT body
  is sealed-box shaped and the plaintext never appears in it; verifies a
  re-sync of unchanged values produces zero new PUTs and zero env-file
  writes; verifies a source-side value change repushes only the affected
  secret. Bitwarden itself is unit-tested against a mock `BwCli`.
- The auth e2e suite (`tests/e2e_auth.rs`) drives the `gh-secrets auth` command
  group and proves the credential precedence end-to-end: `auth status` reports
  provenance without printing values; storing/clearing round-trips through
  `credentials.json` (and the file is `0600`); and — the key assertion — a real
  `manifest sync` against a wiremock GitHub records the exact `Authorization`
  bearer, so each test can confirm the token that *won* (shell env, `.env`,
  `.env.local`, or stored config) is the one that actually reached the API. The
  dotenv parser/precedence planner is also unit-tested in `src/envfile.rs`, and
  the env→stored merge in `src/credentials.rs`.

## Conventional Commits

This repo **squash-merges**, so the PR title is the single commit message that
lands on `master` and the only thing release-please (below) parses. It must be
a [Conventional Commit](https://www.conventionalcommits.org/);
`.github/workflows/pr-lint.yml` enforces that on every PR (a required check).

The allowed type list is defined once and kept in lockstep across three places;
change all three together:

- `.github/workflows/pr-lint.yml` — the `types` of the PR-title check (the
  enforced gate).
- `release-please-config.json` — the `changelog-sections`.
- `.commitlintrc.yml` — the canonical `type-enum`, for local use (`npx
  commitlint`) and as the config a per-commit lint job would consume if the
  merge strategy ever changes to rebase/merge-commit. Not wired into CI today.

Allowed types: `build`, `chore`, `ci`, `docs`, `feat`, `fix`, `perf`,
`refactor`, `revert`, `style`, `test`. `feat` triggers a minor bump, `fix`/
`perf` a patch bump, and a `!` or `BREAKING CHANGE:` footer a major bump.

## Releases and CI secrets

- Releases are **automated from conventional commits** via release-please; do
  not hand-bump `version` or push tags. On every push to `master`,
  `.github/workflows/release.yml` runs release-please, which maintains an open
  "release PR" carrying the next `Cargo.toml`/`Cargo.lock` version bump and the
  generated `CHANGELOG.md`. **Merging that release PR** is the release action:
  it tags `vX.Y.Z`, cuts the GitHub Release, and the same workflow run then
  builds binaries for x86_64 + aarch64 Linux, aarch64 macOS, and x86_64 Windows
  (x86_64 macOS is intentionally omitted — see the matrix comment in
  `release.yml`), attaches each archive with a SHA-256 checksum, and (if
  `CARGO_REGISTRY_TOKEN` is
  configured) publishes to crates.io.
- release-please opens its release PR from the branch
  `release-please--branches--master--components--gh-secrets`, *not* the plain
  `release-please--branches--master`: the rust release-type appends the crate
  name as a component even though `include-component-in-tag` is `false` (that
  setting only strips the component from the `vX.Y.Z` tag, not the branch
  name). Watch for that exact branch name if you poll for the release PR.
- The release build is chained off release-please's `release_created` output in
  the **same** workflow on purpose: a tag pushed by the default `GITHUB_TOKEN`
  does not trigger a separate `push: tags` workflow, so a single workflow is the
  robust design — the build chaining itself needs no PAT (the `RELEASE_PLEASE_TOKEN`
  below is a separate concern, only so the release *PR* gets CI).
  `release-please-manifest.json` is the source of truth for the current version
  — keep it equal to `Cargo.toml`.
- The release PR opens under a PAT (`RELEASE_PLEASE_TOKEN`) when that repo secret
  is set, falling back to `GITHUB_TOKEN` otherwise. This matters because a PR
  opened by `GITHUB_TOKEN` does **not** trigger the `pull_request` CI/lint
  workflows — GitHub suppresses that to avoid recursive runs — so its required
  status checks never appear and **auto-merge can never fire**; the release PR
  has to be merged by hand. A PAT-opened PR triggers CI like any human PR, so
  branch protection is satisfied and auto-merge works. Set it with a PAT (a
  fine-grained token with `contents: read/write` + `pull-requests: read/write`
  on this repo, or a classic `repo` PAT):
  ```
  gh secret set RELEASE_PLEASE_TOKEN --repo <owner>/<repo>
  # paste the token when prompted
  ```
  Without it nothing breaks — the workflow falls back to `GITHUB_TOKEN` and the
  release PR is merged manually (squash, like any release PR).
- Live e2e in CI is gated on a `GH_E2E_TOKEN` repo secret. Set it with a PAT
  that has `repo` scope on the account that should host the sandbox repo:
  ```
  gh secret set GH_E2E_TOKEN --repo <owner>/<repo>
  # paste the token when prompted
  ```
  Without the secret, the `live-e2e` job in `.github/workflows/ci.yml` is a
  no-op. Rotate the PAT through the same command whenever needed.
- `scripts/install.sh` is the cross-platform installer (Linux x86_64 + arm64,
  macOS arm64, Windows x86_64 under a POSIX shell): it detects the host target,
  downloads the matching release archive, verifies its SHA-256, and installs
  the binary. Hosts with no published asset (Intel macOS, non-x86_64 Windows)
  abort with a `cargo install` suggestion rather than 404 on a missing archive,
  so the installer's target set must track the `release.yml` matrix. It must stay in lock-step with the release asset naming in
  `release.yml` — the archive is `gh-secrets-<tag>-<target>.<ext>` and the
  checksum asset is that name with `.sha256` *appended* (it keeps the
  `.tar.gz`/`.zip`), and the binary sits under a leading
  `gh-secrets-<tag>-<target>/` directory inside the archive. The live e2e
  suite (`live_install_script_downloads_and_verifies_release`) runs the script
  against the real release every CI run that has `GH_E2E_TOKEN`, so a drift in
  asset naming fails the gate rather than only surfacing for a user.

## Keeping the allowlist current

- The agent command allowlist lives in `.claude/settings.json`; the tool
  enforces it, so this file does not restate "follow the allowlist."
- Your job is to keep it current: when a new routine command becomes part of
  the normal build/test/release workflow, add it to the allowlist instead of
  re-approving it every session. Keep it narrow.

## Conventions

- One binary (`gh-secrets`) and a thin `lib.rs` that re-exports the modules
  the integration tests need. Production code never depends on the test-only
  `GH_SECRETS_API_BASE` env var being unset — the default value lives in
  `github.rs`.
- Errors use `thiserror` for library errors and `anyhow` only at the CLI
  boundary (`main.rs` / `cli.rs`).
- Time is `chrono::DateTime<Utc>`; serialize as RFC3339. Sync comparisons use
  `>=` so a no-op re-sync of an unchanged secret is genuinely a no-op.
- Do **not** add a `#[ignore]` marker as a way to keep a test out of the
  default gate. If a test is genuinely too expensive to run every time, split
  it into its own recipe that CI still runs (e.g. nightly) and document why
  in this file.

## After the main task: refine and hand off

After completing the user's requested task, look for ways to make future work
easier and propose follow-ups — but only ones that are materially helpful, and
note each one's likely impact:

- **Scripts** — a repeatable step you did by hand that should be automated.
- **`AGENTS.md`** — a constraint, gotcha, or decision worth recording here.
- **Skills** — guidance general enough to belong in a shared skill.
- **Other context** — tests, fixtures, or docs that would improve visibility.

Skip busywork. If nothing is materially helpful, say so and stop.
