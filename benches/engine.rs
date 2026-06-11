//! Criterion micro-benchmarks for the pure sync engine.
//!
//! These measure the in-process work a single `gh-secrets sync`/`check` does
//! between process start and the network: parse the dotenv source
//! (`dotenv_parse`), content-address every value for change detection
//! (`value_hash`), load + validate the manifest (`manifest_load`), and parse
//! the on-disk sync state (`state_parse`). Process startup, credential unlock,
//! and the GitHub/Bitwarden round-trips are deliberately excluded here —
//! `scripts/bench.sh` covers those end to end with hyperfine.
//!
//! The realistic-floor groups use the repo's checked-in `gh-secrets.json` and a
//! small mixed corpus; the `/scaling` and `/synthetic` groups chart how each
//! cost grows with the number of managed secrets (or, for the source read, the
//! number of keys), so a regression that only shows up at scale still surfaces.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

use gh_secrets::envfile::parse_dotenv;
use gh_secrets::manifest::{validate_unique_destination_names, value_hash, Manifest, SyncState};

#[path = "support/mod.rs"]
mod support;

/// SHA-256 content addressing of `name \0 value` — run once per resolved
/// destination name on every sync. Cost scales with the value length, so the
/// corpus spans a token, a ~1 KiB blob, and a ~16 KiB key.
fn bench_value_hash(c: &mut Criterion) {
    let mut group = c.benchmark_group("value_hash");
    for (name, value) in support::value_corpus() {
        group.bench_with_input(BenchmarkId::from_parameter(name), &value, |b, value| {
            b.iter(|| value_hash(black_box("SECRET_NAME"), black_box(value)));
        });
    }
    group.finish();
}

/// Parsing the dotenv text an `env_file` source returns, over a corpus of line
/// shapes (bare, quoted/escaped, and a mixed hand-authored file).
fn bench_dotenv_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("dotenv_parse");
    for (name, content) in support::envfile_corpus() {
        group.bench_with_input(BenchmarkId::from_parameter(name), &content, |b, content| {
            b.iter(|| parse_dotenv(black_box(content)));
        });
    }
    group.finish();
}

/// How the env-source read scales with key count.
fn bench_dotenv_parse_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("dotenv_parse/scaling");
    for n in [10usize, 100, 1000] {
        let content = support::envfile_of(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &content, |b, content| {
            b.iter(|| parse_dotenv(black_box(content)));
        });
    }
    group.finish();
}

/// Manifest load + validation from the checked-in `gh-secrets.json` — the
/// realistic startup cost a config adds to every invocation (filesystem read
/// included, mirroring `Manifest::load`).
fn bench_manifest_load(c: &mut Criterion) {
    let path = support::example_manifest_path();
    c.bench_function("manifest_load/example", |b| {
        b.iter(|| Manifest::load(black_box(&path)).expect("example manifest loads"));
    });
}

/// How manifest parse + validation scales with secret count (parsed from
/// in-memory bytes so the filesystem never enters the measurement).
fn bench_manifest_parse_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("manifest_parse/synthetic");
    for n in [10usize, 100, 1000] {
        let bytes = support::synthetic_manifest_json(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &bytes, |b, bytes| {
            b.iter(|| support::parse_manifest(black_box(bytes)));
        });
    }
    group.finish();
}

/// How the unique-destination-name coherence check scales with secret count —
/// the pure scan re-run on every resolved (and CLI-overridden) secret set.
fn bench_validate_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("validate/synthetic");
    for n in [10usize, 100, 1000] {
        let secrets = support::synthetic_secrets(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &secrets, |b, secrets| {
            b.iter(|| validate_unique_destination_names(black_box(secrets)));
        });
    }
    group.finish();
}

/// How parsing the on-disk sync state scales with secret count — read on every
/// sync/check to drive the "push only when changed" decision.
fn bench_state_parse_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("state_parse/synthetic");
    for n in [10usize, 100, 1000] {
        let bytes = support::synthetic_state_json(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &bytes, |b, bytes| {
            b.iter(|| {
                let state: SyncState =
                    serde_json::from_slice(black_box(bytes)).expect("synthetic state parses");
                state
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_value_hash,
    bench_dotenv_parse,
    bench_dotenv_parse_scaling,
    bench_manifest_load,
    bench_manifest_parse_scaling,
    bench_validate_scaling,
    bench_state_parse_scaling
);
criterion_main!(benches);
