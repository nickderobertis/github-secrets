//! Fixtures shared by the bench targets (`engine`, `engine_allocs`). This file
//! lives in a subdirectory so cargo's bench auto-discovery never treats it as a
//! target of its own; each bench pulls it in with `#[path]`.

// Each bench target uses a subset of these helpers; the unused remainder in any
// one target is expected.
#![allow(dead_code)]

use std::path::PathBuf;

use gh_secrets::manifest::{
    value_hash, EnvFileDestinationConfig, EnvFileSourceConfig, GithubDestinationConfig, Manifest,
    ManifestDestination, ManifestSecret, ManifestSource, SyncState,
};

/// Labelled secret values spanning the size range gh-secrets hashes on every
/// sync: a short API token, a ~1 KiB blob, and a ~16 KiB PEM-shaped key. The
/// content-addressed change check (`value_hash`) is O(value length), so these
/// chart that curve from the common case to the largest a user realistically
/// stores as a single secret.
pub fn value_corpus() -> Vec<(&'static str, String)> {
    vec![
        ("token", format!("ghp_{}", "x".repeat(36))),
        ("medium", "k".repeat(1024)),
        ("large", pem_blob(16 * 1024)),
    ]
}

/// A deterministic PEM-shaped blob of `body_len` base64-alphabet bytes wrapped
/// in key markers — the shape of a TLS private key or a kubeconfig.
fn pem_blob(body_len: usize) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let body: String = (0..body_len)
        .map(|i| ALPHABET[i % ALPHABET.len()] as char)
        .collect();
    format!("-----BEGIN PRIVATE KEY-----\n{body}\n-----END PRIVATE KEY-----\n")
}

/// Labelled dotenv contents covering the line shapes the parser handles: bare
/// assignments, the double-quoted/escaped form our writer emits, and a mixed
/// file with comments, blanks, and `export` prefixes. These are what an
/// `env_file` source costs to read on every sync.
pub fn envfile_corpus() -> Vec<(&'static str, String)> {
    vec![
        (
            "simple",
            "FOO=bar\nBAZ=qux\nAPI_KEY=abcdef0123456789\n".to_string(),
        ),
        (
            "quoted",
            "TOKEN=\"a\\\"b\\\\c\\$d\\`e\"\nMULTILINE=\"line1\\nline2\\nline3\"\n".to_string(),
        ),
        ("mixed", mixed_envfile()),
    ]
}

fn mixed_envfile() -> String {
    [
        "# a hand-authored .env with the usual mess",
        "",
        "export GH_TOKEN=ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        "  NPM_TOKEN = \"npm_yyyyyyyyyyyyyyyyyyyy\" ",
        "# comment in the middle",
        "DATABASE_URL='postgres://user:pass@localhost:5432/db'",
        "",
        "EMPTY=",
        "not an assignment line",
        "MULTILINE=\"first\\nsecond\\nthird\"",
        "",
    ]
    .join("\n")
}

/// A dotenv file of `n` quoted `SECRET_<i>=...` lines, for charting how the
/// env-source read scales with the number of managed keys.
pub fn envfile_of(n: usize) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    for i in 0..n {
        let _ = writeln!(s, "SECRET_{i}=\"value-{i}-{}\"", "x".repeat(24));
    }
    s
}

/// `n` managed secrets with distinct (and therefore collision-free) names —
/// the worst case for `validate_unique_destination_names`, which still has to
/// insert and scan every resolved destination name.
pub fn synthetic_secrets(n: usize) -> Vec<ManifestSecret> {
    (0..n)
        .map(|i| ManifestSecret {
            name: format!("SECRET_{i}"),
            item: None,
            field: None,
            destination_names: Vec::new(),
        })
        .collect()
}

/// A realistic env-file → (github + env-file) manifest carrying `n` secrets.
pub fn synthetic_manifest(n: usize) -> Manifest {
    Manifest {
        source: ManifestSource::EnvFile(EnvFileSourceConfig {
            path: PathBuf::from("source.env"),
        }),
        secrets: synthetic_secrets(n),
        destinations: vec![
            ManifestDestination::Github(GithubDestinationConfig {
                repository: "owner/repo".into(),
            }),
            ManifestDestination::EnvFile(EnvFileDestinationConfig {
                path: PathBuf::from("out.env"),
            }),
        ],
    }
}

/// The serialized JSON for [`synthetic_manifest`] — the exact bytes
/// `Manifest::load` parses, built once outside any timed loop.
pub fn synthetic_manifest_json(n: usize) -> Vec<u8> {
    serde_json::to_vec(&synthetic_manifest(n)).expect("serialize synthetic manifest")
}

/// Parse + validate a manifest from JSON bytes, exactly as `Manifest::load`
/// does after its filesystem read. Asserts success so a fixture that silently
/// stopped parsing can never flatten a scaling curve.
pub fn parse_manifest(bytes: &[u8]) -> Manifest {
    let manifest: Manifest = serde_json::from_slice(bytes).expect("synthetic manifest parses");
    manifest.validate().expect("synthetic manifest validates");
    manifest
}

/// The repo's checked-in `gh-secrets.json` — the same fixture the e2e manifest
/// suite loads, used as the realistic floor for the load benches.
pub fn example_manifest_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("gh-secrets.json")
}

/// A `SyncState` for `n` secrets, each carrying a source hash plus a pushed
/// hash for two destinations — the shape `.gh-secrets-state.json` grows into.
pub fn synthetic_state(n: usize) -> SyncState {
    let mut state = SyncState::default();
    for i in 0..n {
        let name = format!("SECRET_{i}");
        let hash = value_hash(&name, &format!("value-{i}"));
        state.record_source(&name, &hash);
        state.record_push(&name, "github:owner/repo", &hash);
        state.record_push(&name, "env_file:out.env", &hash);
    }
    state
}

/// The serialized JSON for [`synthetic_state`] — the bytes
/// `SyncState::load_or_default` parses on every sync/check.
pub fn synthetic_state_json(n: usize) -> Vec<u8> {
    serde_json::to_vec(&synthetic_state(n)).expect("serialize synthetic state")
}
