# AGENTS.md

Durable instructions for humans and agents working in this repo. Write for a
future maintainer, not as a session log. Put deterministic steps in scripts and
keep this file for constraints, tradeoffs, and judgment.

> `CLAUDE.md` is a symlink to this file (`ln -s AGENTS.md CLAUDE.md`) so the two
> never drift. Edit `AGENTS.md` only.

## What this repo is

`gh-secrets` is a single-binary Rust CLI that syncs secrets from a **source**
to one or more **destinations**, pushing only what changed since the last
sync. There is one workflow built on one internal model: a set of *stores*,
each declaring a read/write capability —

- `github:<owner>/<repo>` — GitHub Actions repository secrets. **Write-only**
  (the API never returns a secret's value), so it is destination-only and the
  CLI rejects `--from github:...` with that explanation.
- `bitwarden` — the Bitwarden vault via the `bw` CLI. Readable today;
  conceptually read/write, but writes are unimplemented and rejected with a
  clear error.
- `env:<path>` — a dotenv-style file. Read/write (`EnvFileSource` /
  `EnvFileDestination` share the same format).
- `local` — the global encrypted store inside the vault (see "Config and
  paths"). Read/write; managed directly with `gh-secrets store
  set|remove|list`.

The pipeline (source → declared secrets → destinations) resolves from a config
file or CLI arguments — `--from`/`--to`/`--secret` each *replace* the
corresponding section of the resolved config (and `--only` filters the
declared set), so any config is reproducible as plain arguments and no config
file is ever required. Config resolution, first hit wins: explicit `--config`
> `--global` > `./gh-secrets.json` (checked in; holds mappings, never values)
> the global config under the config root. That makes `sync`/`check`
project-local inside a project and global elsewhere.

Each managed secret has a *source-side* identity (`name`, plus an optional
`item`/`field` to look up a differently-named source entry) and a
*destination-side* identity (`destination_names`). When `destination_names`
is omitted it defaults to `[name]` — the common "same name everywhere" case
needs no extra config. Supplying it lets the destination name differ from the
source identity and lets one source value fan out to several destination
names (e.g. a single publish token pushed as both `NPM_TOKEN` and
`NODE_AUTH_TOKEN`); the value is fetched once and pushed under each name, and
each (destination-name, destination) pair tracks its own hash in the state
file. Resolved destination names must be unique across the whole config —
two secrets racing to last-writer-wins on one name is a config error,
rejected at load *and* after CLI overrides are applied (so `--secret` args
get the same guard). In argument form, fan-out is expressed as repeated
`--secret DEST=ITEM` entries reading the same item.

Sync is idempotent across runs via a `.gh-secrets-state.json` co-located with
the resolved config (gitignore the project-local one) holding per-(secret,
destination) SHA-256 hashes — the plaintext value is never persisted there.
`check` is the read-only complement: it fetches current source values and
reports what a `sync` would push, judging destinations purely from recorded
state (no GitHub token needed, nothing written — not even the state file).
`list` reports what the config *declares* (name + source item/field, plus
the `-> NAME, NAME` fan-out arrow when set) from the file alone; `source
list` instead *enumerates the source itself* (unlocking it if needed) and
prints item names/ids so a user can discover what to wire in. All of these
honor the never-print-a-value invariant.

## Two standing goals on every task

The user drives product features and their request is the priority — but carry
two goals into *every* task. When either is the lowest-error path to what the
user asked, fold it into the same task without asking first; surface the rest as
follow-ups (see "After the main task").

1. **Engineer the context for next time.** Make the next agent (and you) see
   more for less: realistic end-to-end tests that exercise what the user
   actually sees — especially when they report a bug existing tests missed —
   scripts and skills that automate repetitive steps and shrink their output to
   signal, and terse `AGENTS.md` notes capturing what the code doesn't make
   obvious.
2. **Engineer the codebase and environment.** Be the engineer the user isn't:
   prioritize the technical initiatives that keep the codebase clean,
   maintainable, and repeatable, and keep environment setup automated and
   consistent (`just bootstrap` from a clean clone). Strict quality gates plus
   local/CI parity (same checks, same pinned toolchain) make results
   repeatable — not "works on my machine." A clean base and a reproducible
   environment are usually how the user's feature ships with a low error rate.

## Stack and composition

How this repo was built up from the create-repo skill (dero-skills `v1.47.3`),
recorded so the next maintainer can see why the tooling is what it is.

- **Product shape:** `cli` — a single-binary Rust CLI (`gh-secrets`), installed
  and run as one executable. **Language:** `rust`.
- **References composed:** `shapes/cli.md`, `languages/rust.md`,
  `intersections/rust-cli.md` (this repo is one of its worked examples),
  `project-graph.md`, `ci.md`, `releasing.md` (release-please cuts versions) and
  `llmlint.md` (the LLM-judge tier), on top of `base.md`. `llmlint.yml` is
  composed for exactly that stack (`--shape cli --language rust --releasing`).
- **Excluded or deviating, and why:**
  - *The published crate is the root package, not a member under a virtual
    manifest* (`languages/rust.md` prefers a virtual root). release-please's
    `rust` strategy (package path `.`) bumps `package.version` in the root
    `Cargo.toml` and the root `Cargo.lock`, and refuses a virtual manifest; so
    `gh-secrets` stays the root package that also declares `[workspace]`, its
    `version` stays a literal (not `[workspace.package]`), and release-please
    bumps the members' literal versions in lockstep with it.
  - *The e2e crate takes no Cargo dependency on `gh-secrets`.* The suites only
    spawn the binary, never link the library, so the graph edge is Nx's
    implicit dependency plus `test` `dependsOn gh-secrets:build`.
  - *Live tiers stay in the PR workflow and finish green without their
    secrets* — a standing deviation from `ci.md`'s live-tier rule (own
    workflow, fail fast without the credential). By decision: `live-e2e` and
    `live-e2e-bitwarden` run on pushes and same-repo PRs, `needs` the check
    gate, and `live-e2e-bitwarden` is a required check; making them fail fast
    would turn every unconfigured run red. See "Releases and CI secrets" below.
  - *Release packaging is hand-rolled* tar/zip in `release.yml`, chained off
    release-please in the same workflow, rather than
    `taiki-e/upload-rust-binary-action` behind a tag trigger (a tag pushed by
    `GITHUB_TOKEN` would not trigger a separate workflow). CI's install-path
    job reuses those packaging steps verbatim (checked by
    `tools/check-workflow-contract.mjs`).
  - *Coverage is measured on the Linux and macOS legs only.* On Windows the
    tests still run, uninstrumented, but cargo-llvm-cov there does not
    attribute the spawned binary's coverage, so the number would be wrong.
  - *Supply chain is not part of `just check`*; it runs as its own Linux-only
    CI job (`just supply-chain`), per `languages/rust.md`.
  - *The performance suite* is informational and outside the gate
    (`benches/AGENTS.md`).
  - *Tier thresholds are `ci.md`'s starting defaults* (10 min p95 for the
    affected tier, 5 min for lint/unit) until CI history under the graph gives
    per-target p50/p95; until then nothing is promoted out of the affected tier.

## Command surface

Use the `just` recipes; do not hand-roll equivalent commands. The gate recipes
delegate to Nx (`scripts/nx` runs it on the pinned toolchain) and take a tier:
`affected` (default) or `all` (one full `run-many` sweep).

- `just bootstrap` provisions everything from a clean clone (`just --list`
  says what). Constraints it encodes: rustup >= 1.28 (it installs from
  `rust-toolchain.toml`); bun comes from the `.tool-versions` pin, never
  whatever `bun` is on PATH (`scripts/bun.sh`); Nx runs on Node (any LTS on
  PATH); nextest stays the runner because the inline tests mutate
  process-global env vars and need a process per test.
- `just check [all]` is the gate; `just test` / `lint` / `format-check` run one
  target at the same tier. The affected tier keys off `NX_BASE` (a plain ref
  name or SHA that resolves — anything else is refused before a target runs) or
  else `git merge-base origin/master HEAD`.
- Outside the gate: `just test-live*` (real services; never needed for the
  gate), `just supply-chain` and `just msrv` (own CI jobs; the MSRV is 1.86, the
  floor the locked graph needs), `just lint-llm*` (the judged tier), and
  `just bench*` (`benches/AGENTS.md`).

The product binary is `gh-secrets`. `cargo run -- <args>` invokes it during
development; the e2e projects drive the compiled artifact via `assert_cmd`.

## Project graph

Nx runs targets; Cargo resolves dependencies (one workspace, one `Cargo.lock`).
Each project's `project.json` sits beside its `Cargo.toml`, and each has a
nested `AGENTS.md` for its own rules.

| Project | Dir | Tag | Holds |
| --- | --- | --- | --- |
| `gh-secrets` | `.` | `type:app` | the published crate (lib + bin) and its unit tests |
| `gh-secrets-e2e` | `tests/e2e` | `type:e2e` | offline e2e: wiremock GitHub, stand-in `bw` |
| `gh-secrets-live-github` | `tests/live-github` | `type:live` | real GitHub API + `install.sh` vs the real release |
| `gh-secrets-live-bitwarden` | `tests/live-bitwarden` | `type:live` | real isolated Bitwarden account |
| `gh-secrets-bench` | `benches` | `type:bench` | informational benchmarks |
| `scripts` | `scripts` | `type:tooling` | the repo's scripts (toolchain, gate, installers) and their tests |
| `coverage` | `scripts/coverage` | `type:tooling` | the coverage driver and its end-to-end test |
| `workspace` | `tools` | `type:workspace` | coverage aggregate, supply chain, reconciling checks over root files |

- The root project owns every file no other project claims (`.github/`, the
  justfile, docs), so a change there selects everything; its own inputs are
  narrowed to `src/` and the manifests, so lint/format replay from cache.
  `scripts/` is its own project, so script changes reach only what uses them.
- Boundaries: `tools/project-boundaries.json`, enforced in `workspace:lint` over
  Cargo *and* Nx edges. `type:app` may depend only on `type:app`, so the
  published crate can never be made to depend on a test, live, bench or
  tooling project.
- Coverage: each crate's `test` target runs under cargo-llvm-cov
  (`--no-report`, profiles in `target/llvm-cov-target`); `workspace:coverage`
  merges them and fails below **95%** lines over the gh-secrets crate's `src/`.
  The offline e2e journeys count toward it (they drive the instrumented
  binary), which is how the `bw` wrapper and `main` are covered.

## Invariants (non-negotiable)

- The gate is strict: `clippy` runs with `-D warnings`, `rustfmt` is enforced
  in check mode, and there is no warnings-only mode. A diagnostic is either an
  error or has a tracked rationale.
- Validate all external input at trust boundaries: CLI arguments via clap, the
  on-disk config via serde + explicit field defaults, and GitHub API responses
  via typed structs that reject unknown variants of the small enums we care
  about (visibility, encryption key id, etc.).
- E2E is part of the default gate, not opt-in. The wiremock e2e suite
  (`tests/e2e`) is plain `#[test]`-driven (no `#[ignore]`), spins up a
  mocked GitHub API with `wiremock`, and drives the compiled binary. Live
  GitHub credentials are never required to run the gate. The live e2e suites
  (`tests/live-github` against GitHub, `tests/live-bitwarden` against a
  real isolated Bitwarden account) exist alongside it for opt-in real-API
  coverage — each test runtime-skips with a logged `skip:` line when its gate
  env vars are unset (`GH_SECRETS_LIVE_TEST=1` for both, plus the
  `GH_SECRETS_BW_E2E_*` credentials for the Bitwarden suite), so the default
  gate still compiles and exercises that code path as a no-op (catching
  breakage in the live test helpers without making any network call).
- The CLI never prints the value of a secret to stdout, stderr, log lines, or
  error messages. Secret values are also never written into the configured
  `GH_SECRETS_HOME` path other than inside the encrypted vault (`vault.json`),
  and never in plaintext: the vault's ciphertext envelope is the only at-rest
  form for stored credentials and the `local` store. (`session.json` holds
  time-boxed *key material*, never a credential, a secret value, or the
  passphrase — see "Config and paths".)
- Cross-platform: build and test on Linux, macOS, and Windows in CI.
- Coverage is a gate: 95% lines over the gh-secrets crate, enforced by
  `workspace:coverage` inside `just check`. Never exclude product code to meet it.
- Do not commit secrets, credentials, PII, or customer data.

## Config and paths

The CLI keeps these files under a single root:

- `<root>/vault.json` — the **encrypted vault**: stored credentials
  (`gh-secrets auth`) plus the `local` store's secret values (`gh-secrets
  store`). Envelope encryption: the data is sealed with a random 32-byte data
  key (XChaCha20-Poly1305) and the data key is stored wrapped by an
  Argon2id-derived KEK, so the passphrase is never persisted and the file
  carries only KDF parameters, salt, nonces, and ciphertexts. Unlock order:
  active session > `GH_SECRETS_PASSPHRASE` (shell env or auto-loaded
  `.env`/`.env.local`) > interactive prompt; non-interactive runs with none of
  the three get a precise error, never a hang. The passphrase is cached per
  process, and a *missing* vault never asks for one at all — the engine
  decrypts lazily, only when something actually needs a stored value (so CI
  with `GH_TOKEN` in env never touches it). `0600` on Unix.
- `<root>/session.json` — the vault **session**: the plaintext data key plus
  a hard expiry, minted by `gh-secrets auth unlock` (default 7 days, `--days`
  to change) or automatically by the first prompt-based unlock (announced on
  stderr). Holding this file *is* holding the vault key for its lifetime —
  that is the deliberate convenience/security tradeoff, bounded by `0600`
  permissions and the expiry; it is exactly the `bw unlock` / sudo-timestamp
  model. Because saves reuse the existing wrapped key, a session alone can
  read *and* write the vault. Expired or key-mismatched sessions are deleted
  on sight; `gh-secrets auth lock` deletes it immediately, `auth unlock`
  always re-proves the passphrase (a session cannot extend itself), and
  `auth status` reports the session state.
- `<root>/gh-secrets.json` + `<root>/.gh-secrets-state.json` — the global
  config and its sync state, used when the working directory has no
  project-local config (or `--global` forces it). Same schema as the
  project-local file; scaffold with `gh-secrets init --global`.

The root is resolved as `$GH_SECRETS_HOME` if set, otherwise the platform
config directory (`$XDG_CONFIG_HOME/gh-secrets` on Linux,
`~/Library/Application Support/gh-secrets` on macOS,
`%APPDATA%\gh-secrets` on Windows). Tests use `GH_SECRETS_HOME` pointed at a
tempdir so they never touch the user's real config.

The GitHub API base is `https://api.github.com` and is overridable for tests
via `GH_SECRETS_API_BASE`. That override exists *only* so the e2e suite can
point at `wiremock`; it is intentionally undocumented in `--help`.

Project-local layout and credentials:

- `gh-secrets.json` lives at the repo root (or wherever the user invokes
  `gh-secrets sync` against). It is checked into source control.
- `.gh-secrets-state.json` sits next to it and stores per-(secret,
  destination) SHA-256 hashes that drive the "push only when changed" check.
  **Always gitignore this file** — losing it forces a re-push but leaks
  nothing.
- Credential resolution (the GitHub token, the Bitwarden login, and the vault
  passphrase) follows a single precedence: **shell env > `.env` >
  `.env.local` > stored config**. The credential-consuming commands (`sync`,
  `check`, `source list`, `store`, `auth`) auto-load `.env` then `.env.local`
  from the current directory into the process environment, setting only keys
  that aren't already present — so a real shell variable wins, then `.env`,
  then `.env.local`. The lowest layer is the vault's stored credentials,
  written by `gh-secrets auth github <token>` and `gh-secrets auth bitwarden
  --client-id/--client-secret/--master-password`; `gh-secrets auth status`
  reports where each credential resolves from without ever printing a value,
  and `gh-secrets auth clear [--github|--bitwarden]` removes it. Dotenv
  auto-load is deliberately scoped to those commands, not every invocation
  (`init`/`list` read no credentials): a global load would pull a developer's
  real `.env` into unrelated subprocesses (the test suites run with the repo
  root as CWD, where a real `.env` lives).
- The GitHub destination has no per-config token field by design — the config
  is checked in, the token is not. The token resolves via the precedence
  above (`GH_TOKEN` preferred, then `GITHUB_TOKEN`, then stored config).
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
  precedence above: `sync` auto-loads `.env`/`.env.local`, and any field still
  unset then falls back to the `gh-secrets auth bitwarden` stored config.
  Whatever layer supplies a value, the `bw` subprocess always receives it
  under the canonical `BW_*` name. `BW_SESSION`/`BITWARDEN_SESSION` is the one
  credential never read from stored config — it's an ephemeral unlock token,
  not a durable credential.
- A second test-only override, `GH_SECRETS_TEST_SOURCE_FILE`, points the
  engine's source resolver at a JSON file `{ "NAME": "value", ... }` instead
  of the configured source. Used by `tests/e2e/tests/e2e_manifest.rs` and
  `e2e_auth.rs`, and intentionally undocumented in `--help`, mirroring
  `GH_SECRETS_API_BASE`.

## Scripts and output are context

- Scripts and the CLI itself are quiet on success — a single line, or nothing.
- On failure, print the exact error and a concrete suggested next action to
  stderr, and exit non-zero.
- Treat all command output as context the next agent has to read: maximize
  signal, minimize noise.

## Tests are context engineering

- Tests are how you and future agents actually see this system behave. Invest
  in them deliberately.
- Unit tests cover the pure pieces inline in `src/` (vault crypto in
  `vault.rs`, pipeline resolution and lazy credentials in `engine.rs`, spec
  parsing in `cli.rs`, per-store logic in `sources.rs` / `destinations.rs`,
  the dotenv parser in `envfile.rs`). End-to-end journeys drive the compiled
  binary as a subprocess and live in their own projects — the catalogue of what
  each suite covers is in its nested AGENTS.md (`tests/e2e`,
  `tests/live-github`, `tests/live-bitwarden`). When you touch a feature,
  prefer extending the offline e2e suite — it sees what the user sees, and it
  counts toward the coverage floor.
- The repo's own tooling (tier selection, CI routing, the pre-push hook, the
  boundary and workflow-contract checks, `install.sh --from-dir`) is tested by
  real-subprocess tests in `tools/tests` (`workspace:test`).
- Deliberately *not* e2e-tested: the interactive passphrase prompt and the
  session it auto-mints on a prompted unlock. Those paths need a PTY
  (`rpassword` + `stdin.is_terminal()`), and a PTY harness is flakier than
  the coverage is worth; the logic is unit-tested in `vault.rs` and the
  non-interactive fallbacks (env passphrase, precise no-passphrase error) are
  e2e-tested. Don't bolt a PTY driver onto the suite to close this gap.

## Commits, releases, and merging

### Conventional Commits

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

### Release driver and where each gate tier runs

- **Driver:** release-please in **release-PR mode** (`release.yml`), post-1.0
  bump policy: `feat` → minor, `fix`/`perf` → patch, `!`/`BREAKING CHANGE` →
  major; the other types land without a release. Merging the release PR is the
  only release action (it auto-merges once green; see below).
- **Placement** (`ci.md` "Gate a given commit once"): the release PR can
  accumulate several merges, so this repo *batches* releases and the
  **broader tier — the full `just check all` sweep — runs on the release PR**
  (release-prep), over the exact tree that ships. **Ordinary pull requests and
  pushes to `master` run the affected tier** against an explicit base (the
  merge base with `origin/master`; the previous `master` tip for a push).
  `scripts/ci-gate-tier.mjs` makes that choice in every `check` leg (a release
  PR is a same-repo PR whose head branch starts with
  `release-please--branches--master`), and `tools/tests/ci-gate-tier.test.mjs`
  pins it. `release.yml` builds and publishes the tagged commit and re-gates
  nothing.
- **Fixed status-check contexts** (what branch protection names; keep the job
  ids and matrix values): `check (ubuntu-latest|macos-latest|windows-latest)`,
  `build (…)`, `live-e2e`, `live-e2e-bitwarden`, `llmlint`, `lint PR title`.
  `tools/check-workflow-contract.mjs` fails if one goes missing or gains a
  condition, path filter or `needs` edge that could leave it unreported on a
  PR. The `install (…)`, `supply-chain`, `msrv` and `notignored` jobs are not
  in that set (notignored is a review artifact, never a gate). Which contexts
  are *required* is branch-protection state, applied separately.

### Release pipeline and CI secrets

- Releases are **automated from conventional commits** via release-please; do
  not hand-bump `version` or push tags. On every push to `master`,
  `.github/workflows/release.yml` runs release-please, which maintains an open
  "release PR" carrying the next `Cargo.toml`/`Cargo.lock` version bump and the
  generated `CHANGELOG.md`. That PR has **auto-merge enabled by the workflow**
  (the "Enable auto-merge on the release PR" step), so once its required checks
  go green it merges itself with no human click. Merging it is the release
  action: it tags `vX.Y.Z`, cuts the GitHub Release, and the same workflow run
  then builds binaries for x86_64 + aarch64 Linux, aarch64 macOS, and x86_64
  Windows (x86_64 macOS is intentionally omitted — see the matrix comment in
  `release.yml`), attaches each archive with a SHA-256 checksum, and (if
  `CARGO_REGISTRY_TOKEN` is configured) publishes to crates.io. **Net effect:
  merging one feature/fix PR is the only manual step in shipping a release** —
  release PR, its CI, the merge, the tag, the binaries, and the crates.io
  publish all follow automatically. (Auto-merge requires `RELEASE_PLEASE_TOKEN`
  and the repo's "Allow auto-merge" setting; see below.)
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
  is set, falling back to `GITHUB_TOKEN` otherwise. The PAT does **double duty**,
  and both halves are why the release is fully automatic:
  1. **It opens the release PR**, so the `pull_request` CI/lint workflows run
     on it. A PR opened by `GITHUB_TOKEN` does **not** trigger those workflows
     (GitHub suppresses that to avoid recursive runs), so its required status
     checks never appear and auto-merge could never satisfy branch protection.
  2. **The "Enable auto-merge on the release PR" step uses it** (not
     `GITHUB_TOKEN`) to turn on auto-merge, so when GitHub later performs the
     merge it is attributed to the PAT's user. That attribution is essential:
     the resulting push to `master` must re-trigger `release` to cut the
     tag/Release, and a push by `GITHUB_TOKEN` is ignored by Actions — it would
     strand the release at "PR merged, never tagged".

  Auto-merge also requires the repo's **"Allow auto-merge"** setting to be on
  (Settings → General → Pull Requests) and a branch-protection rule with the
  required status checks; both are already configured on this repo. Set the PAT
  with a fine-grained token (owner of this repo, `contents: read/write` +
  `pull-requests: read/write`) or a classic `repo` PAT:
  ```
  gh secret set RELEASE_PLEASE_TOKEN --repo <owner>/<repo>
  # paste the token when prompted
  ```
  Without it nothing breaks — the workflow falls back to `GITHUB_TOKEN`, the
  auto-merge step logs that it's skipping, and the release PR is merged by hand
  (squash, like any release PR).
- The **llmlint** CI job needs the `OPENAI_API_KEY` repo secret: it
  authenticates the codex CLI, the primary harness in `oneharness.toml`, and the
  job fails naming that secret when it is absent (never a green no-op). The
  model-free `llmlint validate` runs before it and needs nothing.
- Live e2e in CI is gated on a `GH_E2E_TOKEN` repo secret. Set it with a PAT
  that has `repo` scope on the account that should host the sandbox repo:
  ```
  gh secret set GH_E2E_TOKEN --repo <owner>/<repo>
  # paste the token when prompted
  ```
  Without the secret, the GitHub `live-e2e` step in `.github/workflows/ci.yml`
  is a no-op. Rotate the PAT through the same command whenever needed.
- The **isolated Bitwarden e2e account** is a throwaway Bitwarden account used
  only by the live Bitwarden suite. Its api-key credentials live in two places,
  kept in lockstep:
  - In **the maintainer's own Bitwarden vault**, as three secure notes whose
    note body holds the value: `BITWARDEN_TEST_CLIENT_ID`,
    `BITWARDEN_TEST_CLIENT_SECRET`, `BITWARDEN_TEST_MASTER_PASSWORD`. That is
    what `scripts/bw-e2e-env.sh` reads (via `gh-secrets ... --secret
    NAME=ITEM#notes`) to run the suite locally — so a developer never copies the
    isolated account's secret into a shell.
  - As **repo secrets** for CI, so the `live-e2e-bitwarden` job in
    `.github/workflows/ci.yml` can run (it installs the `bw` CLI and runs
    `just test-live-bitwarden`):
    ```
    gh secret set GH_SECRETS_BW_E2E_CLIENT_ID     --repo <owner>/<repo>
    gh secret set GH_SECRETS_BW_E2E_CLIENT_SECRET --repo <owner>/<repo>
    gh secret set GH_SECRETS_BW_E2E_PASSWORD      --repo <owner>/<repo>
    ```
    The client id/secret are a Bitwarden **personal API key** (Account Settings
    → Security → Keys → "View API Key"); the password is the isolated account's
    master password. `live-e2e-bitwarden` is its own job (separate from the
    GitHub `live-e2e`) and a **required status check** on `master`, so a
    Bitwarden regression blocks merges. The run is guarded on
    `GH_SECRETS_BW_E2E_CLIENT_ID` being set, so without the secrets it is
    skipped (not failed) — forks and unconfigured repos stay green.
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
  asset naming fails the gate rather than only surfacing for a user. The
  `install (<os>)` CI job proves the same path on every PR before any release:
  it packages *this commit's* release build with release.yml's own packaging
  steps and installs it with `install.sh --from-dir`, then runs the binary.

## Performance suite

An **informational** suite — Criterion engine benches, hyperfine CLI latency,
deterministic allocation counts and cachegrind instruction counts — run by
`.github/workflows/bench.yml` ("Performance") on every PR with a sticky
comment. It is **deliberately not a gate** (shared-runner timings are noisy) and
never part of `just check` or `just bootstrap`. Layers, recipes and fixture
conventions: `benches/AGENTS.md`. When you add a CLI verb or a hot path, extend
the matching layer there.

## Keeping the allowlist current

- The agent command allowlist lives in `.claude/settings.json`; the tool
  enforces it, so this file does not restate "follow the allowlist."
- Your job is to keep it current: when a new routine command becomes part of
  the normal build/test/release workflow, add it to the allowlist instead of
  re-approving it every session. Keep it narrow.

## Conventions

- One binary (`gh-secrets`) and a thin `lib.rs` that re-exports the modules
  the binary and the bench crate use (the e2e projects never link it). Production code never depends on the test-only
  `GH_SECRETS_API_BASE` env var being unset — the default value lives in
  `github.rs`.
- Errors use `anyhow` with `.context(...)` throughout — every failure the CLI
  surfaces should name the file/operation involved and a concrete next action.
  Introduce `thiserror` only if a typed library error becomes necessary.
- Change detection is content-addressed: a secret is pushed when the SHA-256
  of `name \0 value` differs from the destination's recorded hash, so a no-op
  re-sync of an unchanged secret is genuinely a no-op. Readable destinations
  (env file, local store) additionally compare actual content so out-of-band
  edits are healed rather than trusted.
- Do **not** add a `#[ignore]` marker as a way to keep a test out of the
  default gate. If a suite is genuinely too expensive for every change, split
  it into its own project behind a graph edge unrelated changes cannot reach
  (see "Project graph") and document why in this file.

## After the main task: refine and hand off

After completing the user's requested task, look for ways to make future work
easier and propose follow-ups — but only ones that are materially helpful, and
note each one's likely impact:

- **Scripts** — a repeatable step you did by hand that should be automated.
- **`AGENTS.md`** — a constraint, gotcha, or decision worth recording here.
- **Skills** — guidance general enough to belong in a shared skill.
- **Other context** — tests, fixtures, or docs that would improve visibility.

Skip busywork. If nothing is materially helpful, say so and stop.
