# AGENTS — gh-secrets-bench (the informational performance suite)

The Nx project `gh-secrets-bench` (tag `type:bench`), a `publish = false`
workspace member depending on the `gh-secrets` library. **Never a gate**:
`just check` reaches only its `format-check` and `lint` (clippy
`--all-targets`, so the benches keep compiling); the measurements run from the
`bench*` targets (`just bench`, `bench-cli`, `bench-allocs`,
`bench-instructions`) and `.github/workflows/bench.yml` ("Performance"), which
posts a sticky PR comment and must never become a required check. Bench
targets live in `benches/` under this directory (`benches/benches/*.rs`, so
paths below are relative to this project); `scripts/bench*.sh` drive the
release binary and are listed in this project's inputs.


- Bench the public engine surface (`value_hash`, `parse_dotenv`,
  `Manifest::load`/parse+validate, `SyncState` parse) so the numbers track what
  a `sync`/`check` actually runs, not internals that may be inlined away.
- Load the realistic-floor fixture from the canonical checked-in
  `gh-secrets.json` once, outside every timed loop; never let fixture parsing or
  filesystem I/O leak into a measurement (the `/synthetic` groups parse
  in-memory bytes for exactly this reason).
- Keep the network out entirely. These targets never touch GitHub or Bitwarden;
  the source/destination cost they model is the dotenv parse + SHA-256 change
  detection, and the end-to-end CLI cost (process start, credential unlock) is
  measured separately by `scripts/bench.sh` (hyperfine) and
  `scripts/bench-instructions.sh` (cachegrind).
- Shared fixtures (value corpus, env-file corpus, synthetic manifests/state)
  live in `benches/support/` — a subdirectory so cargo's bench auto-discovery never
  treats the module as a target — and are pulled in via `#[path]`.
- The example fixtures are the realistic floor; scaling groups use synthetic
  worst-case sets. `support::parse_manifest` asserts the manifest parses *and*
  validates, so a fixture that silently stopped parsing can never flatten the
  scaling curve.
- `engine_allocs` reports exact allocator tallies, not time: plain `main`, no
  Criterion, deterministic output for a given commit. Keep it that way — no
  timing, no randomness, no I/O inside a measured closure.
- `cargo check`/`clippy` cover these targets via `--all-targets`; keep them
  warning-clean so they cannot rot. `harness = false` keeps them out of the test
  runner and coverage.

<!-- llmlint: ignore-block[agents_md_durable_and_terse] moved here from the root AGENTS.md as this project's folder-scoped material, as the baseline task directs; condensing the catalogue is the separately scheduled AGENTS.md trim, not part of this move. -->
## The four layers

It has four layers, each measuring a different thing and chosen so that the
ones sensitive to small deltas are the deterministic ones:

- **`benches/engine.rs`** — Criterion micro-benchmarks of the pure in-process
  engine surface a `sync`/`check` runs between process start and the network:
  `value_hash` (SHA-256 content addressing), `parse_dotenv` (the env-file
  source read), `Manifest::load`/parse+validate, and `SyncState` parse. Each
  has a realistic-floor group (the checked-in `gh-secrets.json` / a small
  corpus) and a `/synthetic` (or `/scaling`) group charting cost vs. secret (or
  key) count. `harness = false`; run with `just bench`.
- **`scripts/bench.sh`** (`just bench-cli`) — end-to-end wall-clock latency via
  hyperfine, driving the **release** binary one process per command across the
  offline verbs (`version`, `help`, `list`, `check`, `sync`, `source list`,
  `store list`, `init`). Fully hermetic: an env-file source → env-file
  destination config in a throwaway sandbox, `GH_SECRETS_HOME` and
  `GH_SECRETS_PASSPHRASE` from the environment, so no network, no GitHub token,
  no `bw`. `--dry-run` (`just bench-cli-smoke`) is a one-shot harness smoke
  check used by the workflow.
- **`benches/engine_allocs.rs`** (`just bench-allocs`) — a counting global
  allocator reports exact allocator calls + bytes for the engine hot paths.
  Deterministic for a given commit (`harness = false`, plain `main`, no
  Criterion); no timing/randomness/I/O inside a measured closure.
- **`scripts/bench-instructions.sh`** (`just bench-instructions`) — cachegrind
  instruction counts for the same offline CLI verbs against the **`profiling`**
  profile (release codegen, symbols kept). Linux-only (needs valgrind);
  reproducible to ~0.1%, so this is where a small end-to-end regression is
  trustworthy where a hyperfine delta is not. The `report BASE HEAD` subcommand
  prints a markdown delta table from two `instructions.tsv` files (no valgrind
  needed), which the workflow uses for the base-vs-PR comparison.

The base comparison (Criterion via `critcmp`, plus the instruction-count delta)
is best-effort: when the PR base predates a bench — e.g. the PR that introduces
it — that step fails non-fatally and the report shows absolute numbers only.
The sticky comment is posted on same-repo PRs only (fork PRs get a read-only
token, so they fall back to the job summary + uploaded artifact), keyed by the
`<!-- gh-secrets-perf -->` marker so each run updates one comment in place.

When you add or rename a CLI verb or a hot path, extend the matching layer (a
Criterion group / allocs row for engine code, a hyperfine + cachegrind `measure`
row for a new offline command) so the numbers keep tracking what the binary
runs. The bench-fixture conventions are below.
<!-- llmlint: ignore-end[agents_md_durable_and_terse] -->
