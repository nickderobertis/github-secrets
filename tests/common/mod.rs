// Shared helpers for the e2e tests. `mod common` in a sibling integration test
// file (`tests/e2e.rs`) pulls these in; the `mod.rs` form keeps cargo from
// building this file as a stand-alone test binary.

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
        let mut c = Command::cargo_bin("gh-secrets").expect("locate gh-secrets bin");
        c.env("GH_SECRETS_HOME", self.home.path())
            .env("GH_SECRETS_API_BASE", self.server.uri());
        c
    }
}

/// A 32-byte deterministic value rendered as base64. Real curve validity does
/// not matter for tests because we never decrypt — wiremock only accepts the
/// PUT and asserts on the request shape, not the ciphertext.
pub fn fake_pubkey_b64() -> String {
    B64.encode([42u8; 32])
}
