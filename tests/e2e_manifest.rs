//! End-to-end tests for the manifest-driven sync flow.
//!
//! Drives the compiled `gh-secrets` binary the way a user does, against a
//! mocked GitHub API + a JSON-file "static" source (via
//! `GH_SECRETS_TEST_SOURCE_FILE`). This exercises the orchestrator, source
//! abstraction, env-file destination, and GitHub destination together — same
//! plumbing the user hits when running `gh-secrets sync` in their
//! repo, minus only the Bitwarden CLI call (which is covered by unit tests
//! against a mock `BwCli`).
//!
//! What's covered:
//! - `init` writes a usable starter manifest.
//! - `sync` pushes to both GitHub (one PUT per secret) and the local
//!   env file simultaneously.
//! - The env file ends up with the expected content and preserves unrelated
//!   lines that were already there.
//! - A second `sync` against the same source values is a no-op (no
//!   GitHub PUT, no env file change).
//! - Changing the source value causes a new GitHub PUT and an env-file update.
//! - The PUT body is structurally a sealed-box: base64 of 32 (ephemeral
//!   pubkey) + 16 (MAC) + ciphertext bytes — and the plaintext never appears
//!   in the body. Same guard as `e2e.rs` has for the profile-based sync.

mod common;

use std::fs;
use std::sync::Arc;
use std::sync::Mutex;

use assert_cmd::Command;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use common::fake_pubkey_b64;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use serde_json::{json, Value};
use tempfile::TempDir;
use wiremock::matchers::{header, method, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// Per-test harness: tempdir for working files (manifest, state, source JSON,
/// env destination), wiremock for GitHub, captured PUT bodies for assertions.
struct ManifestHarness {
    dir: TempDir,
    server: MockServer,
    put_bodies: Arc<Mutex<Vec<(String, Value)>>>, // (secret name, JSON body)
}

impl ManifestHarness {
    async fn new() -> Self {
        let dir = TempDir::new().expect("tempdir");
        let server = MockServer::start().await;
        Self {
            dir,
            server,
            put_bodies: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn write_manifest(&self, manifest: &Value) {
        let path = self.dir.path().join("gh-secrets.json");
        fs::write(&path, serde_json::to_vec_pretty(manifest).unwrap()).unwrap();
    }

    fn write_source(&self, values: &Value) {
        let path = self.dir.path().join("source.json");
        fs::write(&path, serde_json::to_vec_pretty(values).unwrap()).unwrap();
    }

    fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("gh-secrets").expect("locate gh-secrets bin");
        c.current_dir(self.dir.path())
            .env(
                "GH_SECRETS_HOME",
                self.dir.path().join("home"), // separate from manifest dir
            )
            .env("GH_SECRETS_API_BASE", self.server.uri())
            .env(
                "GH_SECRETS_TEST_SOURCE_FILE",
                self.dir.path().join("source.json"),
            )
            .env("GH_TOKEN", "ghp_test");
        c
    }

    fn env_file(&self) -> std::path::PathBuf {
        self.dir.path().join(".env")
    }

    fn state_file(&self) -> std::path::PathBuf {
        self.dir.path().join(".gh-secrets-state.json")
    }

    async fn mount_github(&self, repo: &str) {
        let path_re = format!("^/repos/{repo}/actions/secrets/public-key$");
        Mock::given(method("GET"))
            .and(path_regex(path_re))
            .and(header("authorization", "Bearer ghp_test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "key_id": "kid-1",
                "key": fake_pubkey_b64(),
            })))
            .mount(&self.server)
            .await;

        let bodies = self.put_bodies.clone();
        let path_re = format!("^/repos/{repo}/actions/secrets/(.+)$");
        Mock::given(method("PUT"))
            .and(path_regex(path_re))
            .and(header("authorization", "Bearer ghp_test"))
            .respond_with(move |req: &Request| {
                let name = req.url.path().rsplit('/').next().unwrap_or("").to_string();
                let body: Value = serde_json::from_slice(&req.body).expect("PUT body is JSON");
                bodies.lock().unwrap().push((name, body));
                ResponseTemplate::new(201)
            })
            .mount(&self.server)
            .await;
    }
}

fn manifest_for(repo: &str) -> Value {
    json!({
        "source": {"type": "bitwarden"},
        "secrets": [
            {"name": "FOO"},
            {"name": "BAR"}
        ],
        "destinations": [
            {"type": "github", "repository": repo},
            {"type": "env_file", "path": ".env"}
        ]
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_manifest_init_writes_starter() {
    let h = ManifestHarness::new().await;
    h.cmd()
        .args(["init"])
        .assert()
        .success()
        .stdout(contains("wrote starter"));
    let path = h.dir.path().join("gh-secrets.json");
    let body: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(body["source"]["type"], "bitwarden");
    assert!(!body["secrets"].as_array().unwrap().is_empty());
    assert!(!body["destinations"].as_array().unwrap().is_empty());
    // Re-running must refuse to overwrite a non-empty file.
    h.cmd()
        .args(["init"])
        .assert()
        .failure()
        .stderr(contains("refusing to overwrite"));
}

/// `list` reports the managed secret names and their Bitwarden source
/// mapping by reading only the checked-in manifest — no source contact, no
/// credentials, and (since the manifest holds no values) nothing sensitive.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_manifest_list_shows_declared_secrets() {
    let h = ManifestHarness::new().await;
    h.write_manifest(&json!({
        "source": {"type": "bitwarden", "default_field": "api-key"},
        "secrets": [
            {"name": "FOO", "item": "foo-bw-item"},
            {"name": "BAR", "field": "password"}
        ],
        "destinations": [
            {"type": "env_file", "path": ".env"}
        ]
    }));

    h.cmd()
        .args(["list"])
        .assert()
        .success()
        .stdout(contains(
            "secrets (2, config: ./gh-secrets.json, source: bitwarden):",
        ))
        // FOO: explicit item, inherits the source's default field.
        .stdout(contains(
            "FOO  (bitwarden item 'foo-bw-item', field 'api-key')",
        ))
        // BAR: item defaults to the secret name, explicit field overrides.
        .stdout(contains("BAR  (bitwarden item 'BAR', field 'password')"));
}

/// `source list` enumerates the items *available in the source* (here the
/// static test source standing in for the Bitwarden vault) — distinct from
/// `manifest list`, which only echoes what the manifest already declares. It
/// prints each item's name and id and never leaks a value.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_source_list_enumerates_available_items() {
    let h = ManifestHarness::new().await;
    // The manifest declares only FOO, but the source/vault holds more — the
    // whole point of `source list` is to surface the ones you haven't wired up.
    h.write_manifest(&json!({
        "source": {"type": "bitwarden"},
        "secrets": [{"name": "FOO"}],
        "destinations": [{"type": "env_file", "path": ".env"}]
    }));
    h.write_source(&json!({
        "FOO": "foo-secret",
        "STRIPE_KEY": "stripe-secret",
        "DB_PASSWORD": "db-secret"
    }));

    h.cmd()
        .args(["source", "list"])
        .assert()
        .success()
        .stdout(contains("source items (3):"))
        .stdout(contains("STRIPE_KEY"))
        .stdout(contains("DB_PASSWORD"))
        .stdout(contains("FOO"))
        // Never the values.
        .stdout(contains("foo-secret").not())
        .stdout(contains("stripe-secret").not())
        .stdout(contains("db-secret").not());
}

/// A manifest with no declared secrets lists cleanly rather than erroring.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_manifest_list_handles_empty_secrets() {
    let h = ManifestHarness::new().await;
    h.write_manifest(&json!({
        "source": {"type": "bitwarden"},
        "secrets": [],
        "destinations": []
    }));

    h.cmd()
        .args(["list"])
        .assert()
        .success()
        .stdout(contains("no secrets declared"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_manifest_sync_pushes_to_github_and_env_file() {
    let h = ManifestHarness::new().await;
    let repo = "owner/repo1";
    h.write_manifest(&manifest_for(repo));
    h.write_source(&json!({"FOO": "foo-value", "BAR": "bar-value"}));
    h.mount_github(repo).await;
    // Pre-existing line that must survive.
    fs::write(h.env_file(), "PRE_EXISTING=keepme\n").unwrap();

    h.cmd()
        .args(["sync"])
        .assert()
        .success()
        .stdout(contains("created"));

    // GitHub: one PUT per secret, payload is sealed-box shaped, plaintext is
    // not present anywhere in the request body.
    let bodies = h.put_bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    for (name, body) in bodies.iter() {
        assert_eq!(body["key_id"].as_str(), Some("kid-1"));
        let ct_b64 = body["encrypted_value"]
            .as_str()
            .expect("encrypted_value present");
        let raw = B64.decode(ct_b64).expect("encrypted_value is base64");
        // Sealed box: 32-byte ephemeral pubkey + 16-byte MAC + ciphertext.
        assert!(raw.len() > 32 + 16, "{name}: sealed box too short");
        let serialized = serde_json::to_string(body).unwrap();
        assert!(
            !serialized.contains("foo-value") && !serialized.contains("bar-value"),
            "plaintext leaked into PUT body for {name}"
        );
    }
    drop(bodies);

    // Env file: managed keys present + pre-existing line preserved.
    let content = fs::read_to_string(h.env_file()).unwrap();
    assert!(content.contains("PRE_EXISTING=keepme"));
    assert!(content.contains("FOO=\"foo-value\""));
    assert!(content.contains("BAR=\"bar-value\""));

    // State file written, references both destinations.
    let state: Value = serde_json::from_slice(&fs::read(h.state_file()).unwrap()).unwrap();
    let foo_dests = &state["secrets"]["FOO"]["destinations"];
    assert!(foo_dests[format!("github:{repo}")].is_string());
    assert!(foo_dests["env_file:.env"].is_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_manifest_resync_with_unchanged_source_is_a_noop() {
    let h = ManifestHarness::new().await;
    let repo = "owner/repo1";
    h.write_manifest(&manifest_for(repo));
    h.write_source(&json!({"FOO": "v1", "BAR": "v2"}));
    h.mount_github(repo).await;

    h.cmd().args(["sync"]).assert().success();
    assert_eq!(h.put_bodies.lock().unwrap().len(), 2);

    // Second run, same values: should not PUT anything new.
    h.cmd()
        .args(["sync"])
        .assert()
        .success()
        .stdout(contains("nothing to do"));
    assert_eq!(
        h.put_bodies.lock().unwrap().len(),
        2,
        "no new PUTs on the no-op resync"
    );

    // Env file unchanged on disk.
    let content = fs::read_to_string(h.env_file()).unwrap();
    assert!(content.contains("FOO=\"v1\""));
    assert!(content.contains("BAR=\"v2\""));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_manifest_source_change_repushes_only_that_secret() {
    let h = ManifestHarness::new().await;
    let repo = "owner/repo1";
    h.write_manifest(&manifest_for(repo));
    h.write_source(&json!({"FOO": "v1", "BAR": "v2"}));
    h.mount_github(repo).await;

    h.cmd().args(["sync"]).assert().success();
    assert_eq!(h.put_bodies.lock().unwrap().len(), 2);

    // Update only FOO.
    h.write_source(&json!({"FOO": "v1-updated", "BAR": "v2"}));
    h.cmd().args(["sync"]).assert().success();

    // Expect exactly one additional PUT — for FOO.
    let bodies = h.put_bodies.lock().unwrap();
    assert_eq!(bodies.len(), 3);
    assert_eq!(bodies.last().unwrap().0, "FOO");

    // Env file shows the new value for FOO.
    let content = fs::read_to_string(h.env_file()).unwrap();
    assert!(content.contains("FOO=\"v1-updated\""));
    assert!(content.contains("BAR=\"v2\""));
}
