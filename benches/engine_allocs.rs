//! Deterministic allocation report for the sync engine's hot paths.
//!
//! Not a statistical benchmark: a counting global allocator tallies allocator
//! calls and requested bytes for one `value_hash`, `parse_dotenv`, manifest
//! parse+validate, unique-name validation, and state parse per fixture, then
//! prints a markdown table. The counts are exact and stable for a given commit,
//! so two runs are directly comparable — in CI or by eye — without warmups or
//! statistics. They surface allocator pressure, which the wall-clock numbers in
//! `benches/engine.rs` cannot attribute.
//!
//! `harness = false` with a plain `main` keeps libtest, Criterion, nextest, and
//! coverage away from this target (it is measured, not gated). The `--bench`
//! argument cargo passes is deliberately ignored.

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};

use gh_secrets::envfile::parse_dotenv;
use gh_secrets::manifest::{validate_unique_destination_names, value_hash, SyncState};

#[path = "support/mod.rs"]
mod support;

/// The system allocator wrapped with relaxed atomic tallies. A `realloc`
/// counts as one call plus only the grown bytes, so `BYTES` tracks total
/// memory requested without double-counting moves; frees are not tracked.
struct CountingAlloc;

static CALLS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        CALLS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        CALLS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(
            new_size.saturating_sub(layout.size()) as u64,
            Ordering::Relaxed,
        );
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

/// Allocator calls and bytes requested while running `f` (including dropping
/// its result).
fn measure<T>(f: impl FnOnce() -> T) -> (u64, u64) {
    let calls = CALLS.load(Ordering::Relaxed);
    let bytes = BYTES.load(Ordering::Relaxed);
    black_box(f());
    (
        CALLS.load(Ordering::Relaxed) - calls,
        BYTES.load(Ordering::Relaxed) - bytes,
    )
}

fn main() {
    // Flush lazy one-time initialization out of the measured calls so every row
    // reflects steady-state cost.
    for (_, value) in support::value_corpus() {
        black_box(value_hash("SECRET_NAME", &value));
    }

    println!("| operation | case | allocator calls | bytes requested |");
    println!("|---|---|---:|---:|");

    for (name, value) in support::value_corpus() {
        let (calls, bytes) = measure(|| value_hash("SECRET_NAME", &value));
        println!("| value_hash | {name} | {calls} | {bytes} |");
    }

    for (name, content) in support::envfile_corpus() {
        let (calls, bytes) = measure(|| parse_dotenv(&content));
        println!("| dotenv_parse | {name} | {calls} | {bytes} |");
    }

    for n in [10usize, 100, 1000] {
        let bytes_in = support::synthetic_manifest_json(n);
        let (calls, bytes) = measure(|| support::parse_manifest(&bytes_in));
        println!("| manifest_parse | {n} secrets | {calls} | {bytes} |");

        let secrets = support::synthetic_secrets(n);
        let (calls, bytes) = measure(|| validate_unique_destination_names(&secrets));
        println!("| validate | {n} secrets | {calls} | {bytes} |");

        let state_in = support::synthetic_state_json(n);
        let (calls, bytes) = measure(|| {
            serde_json::from_slice::<SyncState>(&state_in).expect("synthetic state parses")
        });
        println!("| state_parse | {n} secrets | {calls} | {bytes} |");
    }
}
