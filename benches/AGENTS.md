# AGENTS — benches

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
  live in `support/` — a subdirectory so cargo's bench auto-discovery never
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
