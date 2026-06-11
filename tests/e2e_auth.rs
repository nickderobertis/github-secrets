//! End-to-end tests for credential auth: the `gh-secrets auth` command group
//! and the resolution precedence `shell env > .env > .env.local > config file`.
//!
//! Drives the compiled binary the way a user does. Two things are proven:
//! 1. `auth status` reports where each credential resolves from — and never
//!    prints the value itself.
//! 2. The precedence actually selects the right GitHub token for a real
//!    `sync`: the token that reaches the (wiremock) GitHub API is the one the
//!    precedence rules say should win.
//! 3. The stored credentials are encrypted at rest: the vault file never
//!    contains a plaintext token, and without the passphrase it cannot be
//!    read (with a precise error, not a hang or a panic).

mod common;

use std::fs;
use std::sync::{Arc, Mutex};

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use serde_json::json;
use tempfile::TempDir;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// Working dir holds `.env`/`.env.local`/manifest/source; `home/` is the
/// isolated config root (so `auth` writes a tempdir vault, never the
/// developer's real one). The vault passphrase is provided via
/// `GH_SECRETS_PASSPHRASE`, exactly as a non-interactive user would. `auth_headers` records the `Authorization` header of
/// every GitHub request so a test can assert which token actually won.
struct AuthHarness {
    dir: TempDir,
    server: MockServer,
    auth_headers: Arc<Mutex<Vec<String>>>,
}

impl AuthHarness {
    async fn new() -> Self {
        let dir = TempDir::new().expect("tempdir");
        let server = MockServer::start().await;
        Self {
            dir,
            server,
            auth_headers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// A command with an isolated config root. Deliberately does NOT set
    /// `GH_TOKEN`/`GITHUB_TOKEN` so each test controls the environment layer
    /// explicitly. `current_dir` is the working dir so dotenv auto-load sees
    /// the files we write there.
    fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("gh-secrets").expect("locate gh-secrets bin");
        c.current_dir(self.dir.path())
            .env("GH_SECRETS_HOME", self.dir.path().join("home"))
            .env("GH_SECRETS_API_BASE", self.server.uri())
            .env("GH_SECRETS_PASSPHRASE", "auth-e2e-passphrase")
            // Scrub any inherited credentials so the test starts from a known
            // state regardless of the developer's shell / .env.
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("BW_CLIENTID")
            .env_remove("BITWARDEN_CLIENT_ID")
            .env_remove("BW_CLIENTSECRET")
            .env_remove("BITWARDEN_CLIENT_SECRET")
            .env_remove("BW_PASSWORD")
            .env_remove("BITWARDEN_MASTER_PASSWORD")
            .env_remove("BITWARDEN_PASSWORD")
            .env_remove("BW_SESSION")
            .env_remove("BITWARDEN_SESSION");
        c
    }

    fn write_file(&self, name: &str, contents: &str) {
        fs::write(self.dir.path().join(name), contents).unwrap();
    }

    fn vault_file(&self) -> std::path::PathBuf {
        self.dir.path().join("home").join("vault.json")
    }

    /// Mount a GitHub mock that accepts any token and records the bearer used.
    async fn mount_github(&self, repo: &str) {
        let pubkey = common::fake_pubkey_b64();
        Mock::given(method("GET"))
            .and(path_regex(format!(
                "^/repos/{repo}/actions/secrets/public-key$"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "key_id": "kid-1",
                "key": pubkey,
            })))
            .mount(&self.server)
            .await;

        let headers = self.auth_headers.clone();
        Mock::given(method("PUT"))
            .and(path_regex(format!("^/repos/{repo}/actions/secrets/(.+)$")))
            .respond_with(move |req: &Request| {
                let auth = req
                    .headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                headers.lock().unwrap().push(auth);
                ResponseTemplate::new(201)
            })
            .mount(&self.server)
            .await;
    }

    fn write_github_manifest(&self, repo: &str) {
        self.write_file(
            "gh-secrets.json",
            &serde_json::to_string_pretty(&json!({
                "source": {"type": "bitwarden"},
                "secrets": [{"name": "FOO"}],
                "destinations": [{"type": "github", "repository": repo}],
            }))
            .unwrap(),
        );
        self.write_file(
            "source.json",
            &serde_json::to_string_pretty(&json!({"FOO": "foo-value"})).unwrap(),
        );
    }

    /// A `sync` command wired to the static-source override so it never
    /// contacts Bitwarden; only the GitHub token resolution is under test.
    fn sync_cmd(&self) -> Command {
        let mut c = self.cmd();
        c.env(
            "GH_SECRETS_TEST_SOURCE_FILE",
            self.dir.path().join("source.json"),
        )
        .args(["sync"]);
        c
    }

    fn bearer_tokens(&self) -> Vec<String> {
        self.auth_headers.lock().unwrap().clone()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_auth_status_reports_unset_then_stored() {
    let h = AuthHarness::new().await;

    // Nothing configured anywhere.
    h.cmd()
        .args(["auth", "status"])
        .assert()
        .success()
        .stdout(contains("GitHub token: not set"))
        .stdout(contains("Bitwarden master password: not set"));

    // Store a token; status now reports it as coming from stored config and the
    // value itself is never printed.
    h.cmd()
        .args(["auth", "github", "ghp_supersecret_value"])
        .assert()
        .success()
        .stdout(contains("stored GitHub token"));

    h.cmd()
        .args(["auth", "status"])
        .assert()
        .success()
        .stdout(contains("GitHub token: set (from stored config)"))
        .stdout(contains("ghp_supersecret_value").not());

    // The vault never contains the plaintext token, and on Unix it is
    // owner-only.
    let raw = fs::read_to_string(h.vault_file()).unwrap();
    assert!(!raw.contains("ghp_supersecret_value"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(h.vault_file()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_auth_bitwarden_stores_only_selected_fields() {
    let h = AuthHarness::new().await;
    h.cmd()
        .args(["auth", "bitwarden", "--client-id", "user.abc"])
        .assert()
        .success()
        .stdout(contains("stored Bitwarden client id"));

    h.cmd()
        .args(["auth", "status"])
        .assert()
        .success()
        .stdout(contains("Bitwarden client id: set (from stored config)"))
        .stdout(contains("Bitwarden client secret: not set"))
        .stdout(contains("user.abc").not());

    // No flags at all is a usage error.
    h.cmd()
        .args(["auth", "bitwarden"])
        .assert()
        .failure()
        .stderr(contains("provide at least one"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_auth_status_provenance_follows_precedence() {
    let h = AuthHarness::new().await;
    // Stored config has a token...
    h.cmd()
        .args(["auth", "github", "from_config"])
        .assert()
        .success();
    // ...and .env also defines one: .env must win over config.
    h.write_file(".env", "GH_TOKEN=from_dotenv\n");
    h.cmd()
        .args(["auth", "status"])
        .assert()
        .success()
        .stdout(contains("GitHub token: set (from the .env file)"));

    // A real shell variable outranks .env.
    h.cmd()
        .env("GH_TOKEN", "from_shell")
        .args(["auth", "status"])
        .assert()
        .success()
        .stdout(contains("GitHub token: set (from your shell environment)"));

    // .env.local is the lowest of the file layers: it only shows through for a
    // key that .env doesn't define.
    h.write_file(".env", "OTHER=1\n");
    h.write_file(".env.local", "GH_TOKEN=from_local\n");
    h.cmd()
        .args(["auth", "status"])
        .assert()
        .success()
        .stdout(contains("GitHub token: set (from the .env.local file)"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_sync_uses_stored_github_token() {
    let h = AuthHarness::new().await;
    let repo = "owner/repo1";
    h.write_github_manifest(repo);
    h.mount_github(repo).await;

    // No token in env or .env — only the stored config has one.
    h.cmd()
        .args(["auth", "github", "ghp_from_stored_config"])
        .assert()
        .success();
    h.sync_cmd().assert().success().stdout(contains("created"));

    let bearers = h.bearer_tokens();
    assert_eq!(bearers.len(), 1, "expected exactly one PUT");
    assert_eq!(bearers[0], "Bearer ghp_from_stored_config");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_sync_dotenv_token_beats_stored_config() {
    let h = AuthHarness::new().await;
    let repo = "owner/repo1";
    h.write_github_manifest(repo);
    h.mount_github(repo).await;

    // Stored config has one token, .env has another — .env must win.
    h.cmd()
        .args(["auth", "github", "ghp_stored_loser"])
        .assert()
        .success();
    h.write_file(".env", "GH_TOKEN=ghp_dotenv_winner\n");

    h.sync_cmd().assert().success().stdout(contains("created"));

    let bearers = h.bearer_tokens();
    assert_eq!(bearers.len(), 1);
    assert_eq!(bearers[0], "Bearer ghp_dotenv_winner");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_sync_shell_env_token_beats_dotenv() {
    let h = AuthHarness::new().await;
    let repo = "owner/repo1";
    h.write_github_manifest(repo);
    h.mount_github(repo).await;

    h.write_file(".env", "GH_TOKEN=ghp_dotenv_loser\n");
    let mut cmd = h.sync_cmd();
    cmd.env("GH_TOKEN", "ghp_shell_winner");
    cmd.assert().success().stdout(contains("created"));

    let bearers = h.bearer_tokens();
    assert_eq!(bearers.len(), 1);
    assert_eq!(bearers[0], "Bearer ghp_shell_winner");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_auth_clear_removes_stored_credentials() {
    let h = AuthHarness::new().await;
    h.cmd().args(["auth", "github", "ghp_x"]).assert().success();
    h.cmd()
        .args(["auth", "bitwarden", "--client-id", "user.x"])
        .assert()
        .success();
    assert!(h.vault_file().exists());

    // Clearing only the GitHub token leaves Bitwarden intact and keeps the file.
    h.cmd()
        .args(["auth", "clear", "--github"])
        .assert()
        .success();
    h.cmd()
        .args(["auth", "status"])
        .assert()
        .success()
        .stdout(contains("GitHub token: not set"))
        .stdout(contains("Bitwarden client id: set (from stored config)"));

    // Clearing everything removes the file entirely.
    h.cmd().args(["auth", "clear"]).assert().success();
    assert!(!h.vault_file().exists());
    h.cmd()
        .args(["auth", "status"])
        .assert()
        .success()
        .stdout(contains("Bitwarden client id: not set"));
}

/// An existing vault without any available passphrase must fail fast with
/// guidance (we run non-interactively, so prompting is impossible) — and a
/// wrong passphrase must produce a decryption error, not garbage.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_vault_requires_the_right_passphrase() {
    let h = AuthHarness::new().await;
    h.cmd().args(["auth", "github", "ghp_x"]).assert().success();

    h.cmd()
        .env_remove("GH_SECRETS_PASSPHRASE")
        .args(["auth", "status"])
        .assert()
        .failure()
        .stderr(contains("GH_SECRETS_PASSPHRASE"));

    h.cmd()
        .env("GH_SECRETS_PASSPHRASE", "not-the-passphrase")
        .args(["auth", "status"])
        .assert()
        .failure()
        .stderr(contains("could not decrypt"));
}

/// When no vault exists at all, credential-consuming commands need no
/// passphrase — the CI path (`GH_TOKEN` in env, nothing stored) must never
/// ask for one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_no_vault_means_no_passphrase_needed() {
    let h = AuthHarness::new().await;
    let repo = "owner/repo1";
    h.write_github_manifest(repo);
    h.mount_github(repo).await;

    let mut cmd = h.sync_cmd();
    cmd.env_remove("GH_SECRETS_PASSPHRASE")
        .env("GH_TOKEN", "ghp_env_only");
    cmd.assert().success().stdout(contains("created"));
    assert_eq!(h.bearer_tokens(), vec!["Bearer ghp_env_only"]);

    // `auth status` likewise reads "not set" everywhere without a vault.
    h.cmd()
        .env_remove("GH_SECRETS_PASSPHRASE")
        .args(["auth", "status"])
        .assert()
        .success()
        .stdout(contains("GitHub token: not set"));
}
