// Shared helpers for the e2e tests. `mod common` in a sibling integration test
// file (`tests/e2e.rs`) pulls these in; the `mod.rs` form keeps cargo from
// building this file as a stand-alone test binary. Not every consumer uses
// every item — silence `dead_code` so partial use doesn't produce warnings.
#![allow(dead_code)]

use std::path::Path;

use assert_cmd::Command;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use tempfile::TempDir;
use wiremock::MockServer;

/// The compiled `gh-secrets` binary aimed at a tempdir for state and a mock
/// GitHub server for API calls. One harness per test keeps the per-profile
/// state and the recorded HTTP calls isolated.
pub struct E2eHarness {
    pub home: TempDir,
    pub server: MockServer,
}

impl E2eHarness {
    pub async fn new() -> Self {
        let home = TempDir::new().expect("create tempdir");
        let server = MockServer::start().await;
        Self { home, server }
    }

    /// A fresh `Command` for the binary with state and API base wired up to
    /// this harness. Spawn one per CLI invocation so env tweaks don't leak.
    pub fn cmd(&self) -> Command {
        let mut c = gh_secrets_cmd();
        c.env("GH_SECRETS_HOME", self.home.path())
            .env("GH_SECRETS_API_BASE", self.server.uri());
        c
    }
}

/// The provider credentials gh-secrets reads from the environment — GitHub
/// (src/destinations.rs `GITHUB_TOKEN_ENVS`) and Bitwarden (src/sources.rs
/// `BW_*_ENVS`). Scrubbed from every spawned command so a developer's real login
/// can never leak into, or be used by, a test; `e2e_auth.rs` checks this list
/// against those declarations. (The vault passphrase is set per harness.)
pub const PROVIDER_CREDENTIAL_ENVS: &[&str] = &[
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "BW_CLIENTID",
    "BITWARDEN_CLIENT_ID",
    "BW_CLIENTSECRET",
    "BITWARDEN_CLIENT_SECRET",
    "BW_PASSWORD",
    "BITWARDEN_MASTER_PASSWORD",
    "BITWARDEN_PASSWORD",
    "BW_SESSION",
    "BITWARDEN_SESSION",
];

/// The credential variable names declared in a source file's `pub const ..._ENVS`
/// lists (string literals and the `BW_*_ENV` constants they reference).
pub fn declared_credential_envs(source: &str) -> Vec<String> {
    let mut consts = std::collections::BTreeMap::new();
    let mut lists = Vec::new();
    for decl in source.split("pub const ").skip(1) {
        let decl = &decl[..decl.find(';').unwrap_or(decl.len())];
        let name = decl.split(':').next().unwrap_or("").trim().to_string();
        let literals: Vec<String> = decl
            .split('"')
            .skip(1)
            .step_by(2)
            .map(String::from)
            .collect();
        if name.ends_with("_ENVS") {
            let idents: Vec<String> = decl
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .filter(|w| w.ends_with("_ENV"))
                .map(String::from)
                .collect();
            lists.push((literals, idents));
        } else if let Some(value) = literals.into_iter().next() {
            consts.insert(name, value);
        }
    }
    let mut names: Vec<String> = lists
        .into_iter()
        .flat_map(|(literals, idents)| {
            literals
                .into_iter()
                .chain(idents.into_iter().filter_map(|i| consts.get(&i).cloned()))
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

/// A 32-byte deterministic value rendered as base64. Real curve validity does
/// not matter for tests because we never decrypt — wiremock only accepts the
/// PUT and asserts on the request shape, not the ciphertext.
pub fn fake_pubkey_b64() -> String {
    B64.encode([42u8; 32])
}

/// A `Command` for the `gh-secrets` binary this run built.
///
/// This crate has no Cargo edge to `gh-secrets` (it spawns the binary and never
/// links the library), so cargo sets no `CARGO_BIN_EXE_gh-secrets` for it. Since
/// Rust 1.100 the test executable no longer sits beside the binary either (it is
/// built under `<target>/<profile>/build/<pkg>/<hash>/out/`), so `assert_cmd`'s
/// `Command::cargo_bin` fallback cannot find it. Instead, take the nearest
/// ancestor of this test executable, inside the run's target dir, that holds the
/// binary: `<target>/<profile>/` under both layouts, and under `cargo llvm-cov`
/// the instrumented copy in `target/llvm-cov-target`. The test target's
/// `dependsOn gh-secrets:build` (and `scripts/coverage/coverage.sh`) builds it.
pub fn gh_secrets_cmd() -> Command {
    let target = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .parent()
        .expect("CARGO_TARGET_TMPDIR has a parent (the target dir)");
    let exe = std::env::current_exe().expect("locate the test executable");
    let name = format!("gh-secrets{}", std::env::consts::EXE_SUFFIX);
    let bin = exe
        .ancestors()
        .skip(1)
        .take_while(|dir| dir.starts_with(target))
        .map(|dir| dir.join(&name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| {
            panic!(
                "no {name} binary under {} on the way up from {}; build it first: \
                 'cargo build -p gh-secrets' (or run the suite through 'just test')",
                target.display(),
                exe.display()
            )
        });
    Command::new(bin)
}
