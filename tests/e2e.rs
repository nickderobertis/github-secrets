//! End-to-end tests for the unified CLI: one `sync` command whose pipeline
//! (source → secrets → destinations) comes from a config file, CLI arguments,
//! or a mix of both.
//!
//! Drives the compiled `gh-secrets` binary the way a user does, against a
//! mocked GitHub API. Sources are real files (env-file source) or the real
//! encrypted vault (local store) — no test-only source override is needed for
//! most of these.
//!
//! What's covered:
//! - A pure-argument pipeline (`--from env:… --to github:… --secret …`)
//!   pushes sealed-box-shaped PUTs and is a no-op on re-run.
//! - `--to` overrides a config's destinations wholesale.
//! - `--only` limits a run to a subset of the declared secrets.
//! - `--from github:…` is rejected: GitHub is a write-only store.
//! - The local store (`store set/list/remove`) round-trips through the
//!   encrypted vault, never leaks plaintext to disk or stdout, and works as
//!   both `--from local` and `--to local`.
//! - `check` reports pending pushes without contacting destinations (no
//!   GitHub token needed) and reports clean after a sync.
//! - With no project-local config, `sync` falls back to the global config;
//!   `init --global` scaffolds it.

mod common;

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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

const PASSPHRASE: &str = "e2e-test-passphrase";

/// Per-test harness: a tempdir as the working directory (configs, env files,
/// state), an isolated config root under `home/`, wiremock for GitHub, and
/// captured PUT bodies for assertions.
struct Harness {
    dir: TempDir,
    server: MockServer,
    put_bodies: Arc<Mutex<Vec<(String, Value)>>>, // (secret name, JSON body)
}

impl Harness {
    async fn new() -> Self {
        Self {
            dir: TempDir::new().expect("tempdir"),
            server: MockServer::start().await,
            put_bodies: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("gh-secrets").expect("locate gh-secrets bin");
        c.current_dir(self.dir.path())
            .env("GH_SECRETS_HOME", self.home())
            .env("GH_SECRETS_API_BASE", self.server.uri())
            .env("GH_SECRETS_PASSPHRASE", PASSPHRASE)
            .env("GH_TOKEN", "ghp_test");
        c
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    fn write(&self, name: &str, contents: &str) {
        let path = self.dir.path().join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.dir.path().join(name)).unwrap()
    }

    async fn mount_github(&self, repo: &str) {
        Mock::given(method("GET"))
            .and(path_regex(format!(
                "^/repos/{repo}/actions/secrets/public-key$"
            )))
            .and(header("authorization", "Bearer ghp_test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "key_id": "kid-1",
                "key": fake_pubkey_b64(),
            })))
            .mount(&self.server)
            .await;

        let bodies = self.put_bodies.clone();
        Mock::given(method("PUT"))
            .and(path_regex(format!("^/repos/{repo}/actions/secrets/(.+)$")))
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

    fn put_count(&self) -> usize {
        self.put_bodies.lock().unwrap().len()
    }
}

/// Assert a PUT body is sealed-box shaped and the plaintext never appears in
/// it — the same structural guard every GitHub-pushing suite carries so a
/// broken seal step can't slip through.
fn assert_sealed_box(name: &str, body: &Value, plaintexts: &[&str]) {
    assert_eq!(body["key_id"].as_str(), Some("kid-1"));
    let ct_b64 = body["encrypted_value"]
        .as_str()
        .expect("encrypted_value present");
    let raw = B64.decode(ct_b64).expect("encrypted_value is base64");
    // Sealed box: 32-byte ephemeral pubkey + 16-byte MAC + ciphertext.
    assert!(raw.len() > 32 + 16, "{name}: sealed box too short");
    let serialized = serde_json::to_string(body).unwrap();
    for plain in plaintexts {
        assert!(
            !serialized.contains(plain),
            "plaintext leaked into PUT body for {name}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_pure_args_env_file_to_github() {
    let h = Harness::new().await;
    let repo = "owner/repo1";
    h.mount_github(repo).await;
    h.write("source.env", "FOO=\"foo-value\"\nBAR=\"bar-value\"\n");

    let args = [
        "sync",
        "--from",
        "env:source.env",
        "--to",
        "github:owner/repo1",
        "--secret",
        "FOO",
        "--secret",
        "BAR",
    ];
    h.cmd()
        .args(args)
        .assert()
        .success()
        .stdout(contains("created"));

    let bodies = h.put_bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    for (name, body) in bodies.iter() {
        assert_sealed_box(name, body, &["foo-value", "bar-value"]);
    }
    drop(bodies);

    // State landed in the working directory (no config file involved) and a
    // re-run with unchanged source values is a no-op.
    assert!(h.dir.path().join(".gh-secrets-state.json").exists());
    h.cmd()
        .args(args)
        .assert()
        .success()
        .stdout(contains("nothing to do"));
    assert_eq!(h.put_count(), 2, "no new PUTs on the no-op resync");

    // A source-side change repushes only the affected secret.
    h.write("source.env", "FOO=\"foo-value2\"\nBAR=\"bar-value\"\n");
    h.cmd().args(args).assert().success();
    let bodies = h.put_bodies.lock().unwrap();
    assert_eq!(bodies.len(), 3);
    assert_eq!(bodies.last().unwrap().0, "FOO");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_to_args_override_config_destinations() {
    let h = Harness::new().await;
    // The config wants to push to GitHub; `--to env:out.env` must replace
    // that destination wholesale, so no PUT may happen.
    h.write(
        "gh-secrets.json",
        &serde_json::to_string_pretty(&json!({
            "source": {"type": "env_file", "path": "source.env"},
            "secrets": [{"name": "FOO"}],
            "destinations": [{"type": "github", "repository": "owner/repo1"}],
        }))
        .unwrap(),
    );
    h.write("source.env", "FOO=value-1\n");

    h.cmd()
        .args(["sync", "--to", "env:out.env"])
        .assert()
        .success()
        .stdout(contains("env_file:out.env: created 'FOO'"));

    assert_eq!(h.put_count(), 0, "github destination was overridden away");
    assert!(h.read("out.env").contains("FOO=\"value-1\""));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_only_limits_the_run_to_named_secrets() {
    let h = Harness::new().await;
    h.write("source.env", "FOO=1\nBAR=2\n");
    let base = [
        "sync",
        "--from",
        "env:source.env",
        "--to",
        "env:out.env",
        "--secret",
        "FOO",
        "--secret",
        "BAR",
    ];

    h.cmd()
        .args(base)
        .args(["--only", "FOO"])
        .assert()
        .success();
    let out = h.read("out.env");
    assert!(out.contains("FOO="));
    assert!(!out.contains("BAR="), "BAR was filtered out by --only");

    // An --only name that isn't declared is an error, not a silent no-op.
    h.cmd()
        .args(base)
        .args(["--only", "MISSING"])
        .assert()
        .failure()
        .stderr(contains("MISSING"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_github_as_source_is_rejected() {
    let h = Harness::new().await;
    h.cmd()
        .args(["sync", "--from", "github:owner/repo1", "--to", "env:.env"])
        .assert()
        .failure()
        .stderr(contains("write-only"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_store_round_trips_encrypted_and_feeds_sync() {
    let h = Harness::new().await;
    let repo = "owner/repo1";
    h.mount_github(repo).await;

    // Set one value as an argument and one via stdin.
    h.cmd()
        .args(["store", "set", "FOO", "foo-store-value"])
        .assert()
        .success()
        .stdout(contains("set 'FOO'").and(contains("foo-store-value").not()));
    h.cmd()
        .args(["store", "set", "BAR"])
        .write_stdin("bar-store-value\n")
        .assert()
        .success()
        .stdout(contains("set 'BAR'"));

    // Names only — never values.
    h.cmd()
        .args(["store", "list"])
        .assert()
        .success()
        .stdout(contains("FOO"))
        .stdout(contains("BAR"))
        .stdout(contains("foo-store-value").not())
        .stdout(contains("bar-store-value").not());

    // The vault is encrypted at rest: neither names nor values are readable.
    let raw = fs::read_to_string(h.home().join("vault.json")).unwrap();
    assert!(!raw.contains("foo-store-value"));
    assert!(!raw.contains("bar-store-value"));
    assert!(!raw.contains("FOO"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(h.home().join("vault.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // The store works as a source.
    h.cmd()
        .args([
            "sync",
            "--from",
            "local",
            "--to",
            "github:owner/repo1",
            "--secret",
            "FOO",
        ])
        .assert()
        .success()
        .stdout(contains(format!("github:{repo}: created 'FOO'")));
    let bodies = h.put_bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    assert_sealed_box("FOO", &bodies[0].1, &["foo-store-value"]);
    drop(bodies);

    // Remove; a removed name is gone and removing it again errors.
    h.cmd().args(["store", "remove", "BAR"]).assert().success();
    h.cmd()
        .args(["store", "remove", "BAR"])
        .assert()
        .failure()
        .stderr(contains("no secret named 'BAR'"));
    h.cmd()
        .args(["store", "list"])
        .assert()
        .success()
        .stdout(contains("BAR").not());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_local_store_as_destination() {
    let h = Harness::new().await;
    h.write("source.env", "FOO=imported-value\n");
    h.cmd()
        .args([
            "sync",
            "--from",
            "env:source.env",
            "--to",
            "local",
            "--secret",
            "FOO",
        ])
        .assert()
        .success()
        .stdout(contains("local: created 'FOO'"));

    h.cmd()
        .args(["store", "list"])
        .assert()
        .success()
        .stdout(contains("FOO"));

    // Idempotent: a re-run with the same source value is a no-op.
    h.cmd()
        .args([
            "sync",
            "--from",
            "env:source.env",
            "--to",
            "local",
            "--secret",
            "FOO",
        ])
        .assert()
        .success()
        .stdout(contains("nothing to do"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_check_reports_pending_then_clean_without_pushing() {
    let h = Harness::new().await;
    let repo = "owner/repo1";
    h.mount_github(repo).await;
    h.write(
        "gh-secrets.json",
        &serde_json::to_string_pretty(&json!({
            "source": {"type": "env_file", "path": "source.env"},
            "secrets": [{"name": "FOO"}, {"name": "BAR"}],
            "destinations": [{"type": "github", "repository": repo}],
        }))
        .unwrap(),
    );
    h.write("source.env", "FOO=1\nBAR=2\n");

    // Check needs no GitHub token: it never contacts destinations.
    h.cmd()
        .env_remove("GH_TOKEN")
        .args(["check"])
        .assert()
        .success()
        .stdout(contains(format!("github:{repo}: 2 to push (FOO, BAR)")))
        .stdout(contains("2 push(es) pending"));
    assert_eq!(h.put_count(), 0, "check must not push");

    h.cmd().args(["sync"]).assert().success();
    assert_eq!(h.put_count(), 2);

    h.cmd()
        .env_remove("GH_TOKEN")
        .args(["check"])
        .assert()
        .success()
        .stdout(contains("everything is up to date"));
    assert_eq!(h.put_count(), 2, "check after sync must not push either");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_falls_back_to_global_config() {
    let h = Harness::new().await;
    // Global config under the config root; paths inside it resolve relative
    // to that directory.
    let source = h.dir.path().join("global-source.env");
    fs::write(&source, "FOO=global-value\n").unwrap();
    let out = h.dir.path().join("global-out.env");
    fs::create_dir_all(h.home()).unwrap();
    fs::write(
        h.home().join("gh-secrets.json"),
        serde_json::to_string_pretty(&json!({
            "source": {"type": "env_file", "path": source},
            "secrets": [{"name": "FOO"}],
            "destinations": [{"type": "env_file", "path": out}],
        }))
        .unwrap(),
    )
    .unwrap();

    // The working directory has no gh-secrets.json, so the global config is
    // used — and its state lives next to it, under the config root.
    h.cmd().args(["sync"]).assert().success();
    assert!(fs::read_to_string(&out).unwrap().contains("global-value"));
    assert!(h.home().join(".gh-secrets-state.json").exists());

    // A project-local config would normally win; --global forces the global
    // one even then.
    h.write(
        "gh-secrets.json",
        &serde_json::to_string_pretty(&json!({
            "source": {"type": "env_file", "path": "local-source.env"},
            "secrets": [{"name": "LOCAL_ONLY"}],
            "destinations": [{"type": "env_file", "path": "local-out.env"}],
        }))
        .unwrap(),
    );
    h.cmd()
        .args(["check", "--global"])
        .assert()
        .success()
        .stdout(contains("everything is up to date"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_init_scaffolds_local_and_global() {
    let h = Harness::new().await;
    h.cmd()
        .args(["init"])
        .assert()
        .success()
        .stdout(contains("wrote starter"));
    let local: Value =
        serde_json::from_str(&h.read("gh-secrets.json")).expect("starter is valid JSON");
    assert_eq!(local["source"]["type"], "bitwarden");
    // Re-running must refuse to overwrite.
    h.cmd()
        .args(["init"])
        .assert()
        .failure()
        .stderr(contains("refusing to overwrite"));

    h.cmd()
        .args(["init", "--global"])
        .assert()
        .success()
        .stdout(contains("wrote starter"));
    let global: Value =
        serde_json::from_str(&fs::read_to_string(h.home().join("gh-secrets.json")).unwrap())
            .unwrap();
    // The global starter sources from the encrypted local store.
    assert_eq!(global["source"]["type"], "local");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_list_shows_mapping_for_env_file_source() {
    let h = Harness::new().await;
    h.write(
        "gh-secrets.json",
        &serde_json::to_string_pretty(&json!({
            "source": {"type": "env_file", "path": ".env.master"},
            "secrets": [
                {"name": "DATABASE_URL", "item": "DB_URL"},
                {"name": "FOO"}
            ],
            "destinations": [{"type": "github", "repository": "owner/repo1"}],
        }))
        .unwrap(),
    );
    h.cmd()
        .args(["list"])
        .assert()
        .success()
        .stdout(contains("secrets (2"))
        .stdout(contains("source: env file"))
        .stdout(contains(
            "DATABASE_URL  (env file '.env.master', key 'DB_URL')",
        ))
        .stdout(contains("FOO  (env file '.env.master', key 'FOO')"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_source_list_enumerates_env_file_and_local_store() {
    let h = Harness::new().await;
    h.write("source.env", "ALPHA=a-value\nBETA=b-value\n");
    h.cmd()
        .args(["source", "list", "--from", "env:source.env"])
        .assert()
        .success()
        .stdout(contains("source items (2):"))
        .stdout(contains("ALPHA"))
        .stdout(contains("BETA"))
        .stdout(contains("a-value").not())
        .stdout(contains("b-value").not());

    h.cmd()
        .args(["store", "set", "GAMMA", "g-value"])
        .assert()
        .success();
    h.cmd()
        .args(["source", "list", "--from", "local"])
        .assert()
        .success()
        .stdout(contains("GAMMA"))
        .stdout(contains("g-value").not());
}

/// An explicit `--config` path is honored wherever it lives: relative paths
/// inside the config resolve against the config's directory (not the CWD),
/// and the state file lands next to the config. A nonexistent path is an
/// error, not a silent fall-through to another config.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_explicit_config_path_resolves_relative_to_itself() {
    let h = Harness::new().await;
    h.write(
        "proj/config.json",
        &serde_json::to_string_pretty(&json!({
            "source": {"type": "env_file", "path": "src.env"},
            "secrets": [{"name": "FOO"}],
            "destinations": [{"type": "env_file", "path": "out.env"}],
        }))
        .unwrap(),
    );
    h.write("proj/src.env", "FOO=from-subdir\n");

    h.cmd()
        .args(["sync", "--config", "proj/config.json"])
        .assert()
        .success()
        .stdout(contains("created 'FOO'"));
    // Both the destination and the state resolved against proj/, not the CWD.
    assert!(h.read("proj/out.env").contains("from-subdir"));
    assert!(h.dir.path().join("proj/.gh-secrets-state.json").exists());
    assert!(!h.dir.path().join(".gh-secrets-state.json").exists());

    h.cmd()
        .args(["check", "--config", "proj/config.json"])
        .assert()
        .success()
        .stdout(contains("everything is up to date"));

    h.cmd()
        .args(["sync", "--config", "missing.json"])
        .assert()
        .failure()
        .stderr(contains("does not exist"));
}

/// `--state` redirects the idempotency bookkeeping: the run writes exactly
/// there, and a re-run against the same state is the usual no-op.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_state_flag_overrides_state_location() {
    let h = Harness::new().await;
    h.write("source.env", "FOO=1\n");
    let args = [
        "sync",
        "--from",
        "env:source.env",
        "--to",
        "env:out.env",
        "--secret",
        "FOO",
        "--state",
        "custom/state.json",
    ];
    h.cmd().args(args).assert().success();
    assert!(h.dir.path().join("custom/state.json").exists());
    assert!(!h.dir.path().join(".gh-secrets-state.json").exists());
    h.cmd()
        .args(args)
        .assert()
        .success()
        .stdout(contains("nothing to do"));
}

/// `init --path` writes the starter wherever asked, and `init --global`
/// refuses to clobber an existing global config just like the local form.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_init_honors_path_and_refuses_global_overwrite() {
    let h = Harness::new().await;
    h.cmd()
        .args(["init", "--path", "nested/custom.json"])
        .assert()
        .success()
        .stdout(contains("nested/custom.json"));
    let parsed: Value =
        serde_json::from_str(&h.read("nested/custom.json")).expect("starter is valid JSON");
    assert_eq!(parsed["source"]["type"], "bitwarden");

    h.cmd().args(["init", "--global"]).assert().success();
    h.cmd()
        .args(["init", "--global"])
        .assert()
        .failure()
        .stderr(contains("refusing to overwrite"));
}

/// `list --global` reads the global config even from inside a project, and
/// renders the local-store source mapping.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_list_global_shows_local_store_mapping() {
    let h = Harness::new().await;
    fs::create_dir_all(h.home()).unwrap();
    fs::write(
        h.home().join("gh-secrets.json"),
        serde_json::to_string_pretty(&json!({
            "source": {"type": "local"},
            "secrets": [{"name": "RENAMED", "item": "STORE_KEY"}, {"name": "PLAIN"}],
            "destinations": [{"type": "github", "repository": "owner/repo1"}],
        }))
        .unwrap(),
    )
    .unwrap();
    // A project-local config exists too; --global must skip it.
    h.write(
        "gh-secrets.json",
        &serde_json::to_string_pretty(&json!({
            "source": {"type": "bitwarden"},
            "secrets": [{"name": "LOCAL_ONLY"}],
            "destinations": [],
        }))
        .unwrap(),
    );

    h.cmd()
        .args(["list", "--global"])
        .assert()
        .success()
        .stdout(contains("source: local store"))
        .stdout(contains("RENAMED  (local store key 'STORE_KEY')"))
        .stdout(contains("PLAIN  (local store key 'PLAIN')"))
        .stdout(contains("LOCAL_ONLY").not());
}

/// `--secret NAME=ITEM` remaps the destination name onto a different source
/// key, all the way through a real sync.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_secret_item_mapping_renames_through_sync() {
    let h = Harness::new().await;
    h.write("source.env", "DB_URL=postgres://srv/db\n");
    h.cmd()
        .args([
            "sync",
            "--from",
            "env:source.env",
            "--to",
            "env:out.env",
            "--secret",
            "DATABASE_URL=DB_URL",
        ])
        .assert()
        .success()
        .stdout(contains("created 'DATABASE_URL'"));
    assert!(h
        .read("out.env")
        .contains("DATABASE_URL=\"postgres://srv/db\""));
}

/// Error surfaces a user will actually hit: a declared secret missing from
/// the source names the key, Bitwarden scoping flags on a non-bitwarden
/// source are rejected (not silently dropped), and a pipeline with no
/// declared secrets is a clean no-op.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_sync_error_and_noop_edges() {
    let h = Harness::new().await;
    h.write("source.env", "FOO=1\n");

    h.cmd()
        .args([
            "sync",
            "--from",
            "env:source.env",
            "--to",
            "env:out.env",
            "--secret",
            "NOPE",
        ])
        .assert()
        .failure()
        .stderr(contains("has no value for 'NOPE'"));

    h.cmd()
        .args([
            "sync",
            "--from",
            "env:source.env",
            "--to",
            "env:out.env",
            "--secret",
            "FOO",
            "--collection-id",
            "coll-1",
        ])
        .assert()
        .failure()
        .stderr(contains("--from bitwarden"));

    // No secrets declared anywhere: nothing to do, and no files appear.
    h.cmd()
        .args(["sync", "--from", "env:source.env", "--to", "env:out.env"])
        .assert()
        .success()
        .stdout(contains("nothing to do"));
    assert!(!h.dir.path().join("out.env").exists());

    // Two --secret args claiming the same destination name get the same
    // uniqueness guard a config file does.
    h.cmd()
        .args([
            "sync",
            "--from",
            "env:source.env",
            "--to",
            "env:out.env",
            "--secret",
            "FOO",
            "--secret",
            "FOO=other-item",
        ])
        .assert()
        .failure()
        .stderr(contains("destination name 'FOO'"));

    // The store rejects an empty name before touching the vault.
    h.cmd()
        .args(["store", "set", "", "value"])
        .assert()
        .failure()
        .stderr(contains("name cannot be empty"));
}

/// GitHub error statuses surface the precise, actionable message from
/// `github.rs` — both on the PUT and on the public-key GET — instead of a
/// generic HTTP failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_github_error_statuses_surface_actionable_messages() {
    let cases: [(u16, &str); 4] = [
        (401, "set a valid token"),
        (403, "lacks the required permissions"),
        (404, "check the repository name"),
        (500, "GitHub request failed"),
    ];

    // Failures on the PUT itself.
    for (status, phrase) in cases {
        let h = Harness::new().await;
        h.write("source.env", "FOO=1\n");
        Mock::given(method("GET"))
            .and(path_regex(
                "^/repos/owner/repo1/actions/secrets/public-key$",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "key_id": "kid-1",
                "key": fake_pubkey_b64(),
            })))
            .mount(&h.server)
            .await;
        Mock::given(method("PUT"))
            .and(path_regex("^/repos/owner/repo1/actions/secrets/(.+)$"))
            .respond_with(ResponseTemplate::new(status))
            .mount(&h.server)
            .await;
        h.cmd()
            .args(sync_args())
            .assert()
            .failure()
            .stderr(contains(status.to_string()))
            .stderr(contains(phrase))
            .stderr(contains("uploading 'FOO' to owner/repo1"));
    }

    // Failures fetching the public key get their own context.
    for (status, phrase) in cases {
        let h = Harness::new().await;
        h.write("source.env", "FOO=1\n");
        Mock::given(method("GET"))
            .and(path_regex(
                "^/repos/owner/repo1/actions/secrets/public-key$",
            ))
            .respond_with(ResponseTemplate::new(status))
            .mount(&h.server)
            .await;
        h.cmd()
            .args(sync_args())
            .assert()
            .failure()
            .stderr(contains(status.to_string()))
            .stderr(contains(phrase))
            .stderr(contains("fetching public key for owner/repo1"));
    }
}

/// The `sync` args every GitHub error-path test shares: one secret from an
/// env-file source to the mocked `owner/repo1`.
fn sync_args() -> [&'static str; 7] {
    [
        "sync",
        "--from",
        "env:source.env",
        "--to",
        "github:owner/repo1",
        "--secret",
        "FOO",
    ]
}

/// A PUT answered with 204 (secret already existed) is reported as "updated",
/// not "created" — the only place the created/updated wire distinction shows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_put_204_reports_updated() {
    let h = Harness::new().await;
    h.write("source.env", "FOO=1\n");
    Mock::given(method("GET"))
        .and(path_regex(
            "^/repos/owner/repo1/actions/secrets/public-key$",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "key_id": "kid-1",
            "key": fake_pubkey_b64(),
        })))
        .mount(&h.server)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex("^/repos/owner/repo1/actions/secrets/(.+)$"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&h.server)
        .await;

    h.cmd()
        .args(sync_args())
        .assert()
        .success()
        .stdout(contains("github:owner/repo1: updated 'FOO'"))
        .stdout(contains("0 created, 1 updated, 0 unchanged"));
}

/// A malformed public-key response is rejected at the trust boundary with a
/// precise error — wrong key length and non-JSON body each name the problem.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_malformed_public_key_is_a_precise_error() {
    // 31 bytes is not a valid X25519 public key.
    let h = Harness::new().await;
    h.write("source.env", "FOO=1\n");
    Mock::given(method("GET"))
        .and(path_regex(
            "^/repos/owner/repo1/actions/secrets/public-key$",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "key_id": "kid-1",
            "key": B64.encode([7u8; 31]),
        })))
        .mount(&h.server)
        .await;
    h.cmd()
        .args(sync_args())
        .assert()
        .failure()
        .stderr(contains("wrong length"));

    // A body that isn't the documented JSON shape fails parsing, with context.
    let h = Harness::new().await;
    h.write("source.env", "FOO=1\n");
    Mock::given(method("GET"))
        .and(path_regex(
            "^/repos/owner/repo1/actions/secrets/public-key$",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&h.server)
        .await;
    h.cmd()
        .args(sync_args())
        .assert()
        .failure()
        .stderr(contains("parsing public-key for owner/repo1"));
}

/// When one destination fails mid-sync, no state is recorded for *any*
/// destination — so the next run re-pushes everything rather than silently
/// believing a push that never landed. The earlier destination's write stands
/// (its content is the source of truth on the re-run).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_failed_destination_records_no_state_so_rerun_repushes() {
    let h = Harness::new().await;
    let repo = "owner/repo1";
    // Exactly one 500, mounted at higher priority than the recording PUT the
    // harness mounts below; every later PUT succeeds.
    Mock::given(method("PUT"))
        .and(path_regex(format!("^/repos/{repo}/actions/secrets/(.+)$")))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&h.server)
        .await;
    h.mount_github(repo).await;
    // env destination first, github second: the env write succeeds before the
    // github destination aborts the run.
    h.write(
        "gh-secrets.json",
        &serde_json::to_string_pretty(&json!({
            "source": {"type": "env_file", "path": "source.env"},
            "secrets": [{"name": "FOO"}],
            "destinations": [
                {"type": "env_file", "path": "out.env"},
                {"type": "github", "repository": repo},
            ],
        }))
        .unwrap(),
    );
    h.write("source.env", "FOO=v1\n");

    h.cmd()
        .args(["sync"])
        .assert()
        .failure()
        .stderr(contains(format!("applying to destination github:{repo}")));
    assert!(h.read("out.env").contains("FOO=\"v1\""), "env write landed");
    assert!(
        !h.dir.path().join(".gh-secrets-state.json").exists(),
        "a failed sync must not record state"
    );

    // The re-run pushes to GitHub and converges; a third run is the no-op.
    h.cmd().args(["sync"]).assert().success();
    assert_eq!(h.put_count(), 1, "the re-run repushed the failed secret");
    h.cmd()
        .args(["sync"])
        .assert()
        .success()
        .stdout(contains("nothing to do"));
    assert_eq!(h.put_count(), 1);
}

/// The state file holds hashes, never values — and deleting it merely forces
/// a re-push (losing it leaks nothing and breaks nothing).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_state_file_holds_no_values_and_deleting_it_forces_repush() {
    let h = Harness::new().await;
    let repo = "owner/repo1";
    h.mount_github(repo).await;
    h.write("source.env", "FOO=\"state-secret-value\"\n");
    let args = [
        "sync",
        "--from",
        "env:source.env",
        "--to",
        "github:owner/repo1",
        "--secret",
        "FOO",
    ];
    h.cmd().args(args).assert().success();
    assert_eq!(h.put_count(), 1);

    let state = h.read(".gh-secrets-state.json");
    assert!(
        !state.contains("state-secret-value"),
        "plaintext leaked into the state file"
    );
    assert!(
        state.contains(&format!("github:{repo}")),
        "state keys on the destination"
    );

    fs::remove_file(h.dir.path().join(".gh-secrets-state.json")).unwrap();
    h.cmd().args(args).assert().success();
    assert_eq!(h.put_count(), 2, "lost state forces a re-push");
    h.cmd()
        .args(args)
        .assert()
        .success()
        .stdout(contains("nothing to do"));
    assert_eq!(h.put_count(), 2);
}

/// Out-of-band edits to a readable destination are healed by `sync` even when
/// the recorded state says nothing changed — while `check` (state-only by
/// design) keeps reporting clean. Unrelated lines survive every heal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_sync_heals_out_of_band_env_edits_while_check_stays_state_only() {
    let h = Harness::new().await;
    h.write("source.env", "FOO=canonical\n");
    h.write("out.env", "# pinned comment\nOTHER=keepme\n");
    let sync = [
        "sync",
        "--from",
        "env:source.env",
        "--to",
        "env:out.env",
        "--secret",
        "FOO",
    ];
    let check = [
        "check",
        "--from",
        "env:source.env",
        "--to",
        "env:out.env",
        "--secret",
        "FOO",
    ];
    h.cmd().args(sync).assert().success();
    assert!(h.read("out.env").contains("FOO=\"canonical\""));

    // Tamper with the managed line: check judges from state alone and stays
    // clean; sync compares content and rewrites the canonical line.
    h.write(
        "out.env",
        "# pinned comment\nOTHER=keepme\nFOO=\"tampered\"\n",
    );
    h.cmd()
        .args(check)
        .assert()
        .success()
        .stdout(contains("everything is up to date"));
    h.cmd()
        .args(sync)
        .assert()
        .success()
        .stdout(contains("updated 'FOO'"));
    let healed = h.read("out.env");
    assert!(healed.contains("FOO=\"canonical\""));
    assert!(healed.contains("# pinned comment"));
    assert!(healed.contains("OTHER=keepme"));

    // Delete the managed line entirely: sync re-creates it.
    h.write("out.env", "# pinned comment\nOTHER=keepme\n");
    h.cmd()
        .args(sync)
        .assert()
        .success()
        .stdout(contains("created 'FOO'"));
    assert!(h.read("out.env").contains("FOO=\"canonical\""));

    // Converged again: the next run is a genuine no-op.
    h.cmd()
        .args(sync)
        .assert()
        .success()
        .stdout(contains("nothing to do"));
}

/// The local store is the other readable destination: a value changed behind
/// the sync's back (`store set`) is healed on the next run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_sync_heals_local_store_drift() {
    let h = Harness::new().await;
    h.write("source.env", "FOO=canonical\n");
    let sync = [
        "sync",
        "--from",
        "env:source.env",
        "--to",
        "local",
        "--secret",
        "FOO",
    ];
    h.cmd().args(sync).assert().success();
    h.cmd()
        .args(["store", "set", "FOO", "drifted"])
        .assert()
        .success();

    h.cmd()
        .args(sync)
        .assert()
        .success()
        .stdout(contains("updated 'FOO'"));
    h.cmd()
        .args(sync)
        .assert()
        .success()
        .stdout(contains("nothing to do"));

    // Read the healed value back out through the store-as-source path.
    h.cmd()
        .args([
            "sync",
            "--from",
            "local",
            "--to",
            "env:verify.env",
            "--secret",
            "FOO",
        ])
        .assert()
        .success();
    assert!(h.read("verify.env").contains("FOO=\"canonical\""));
}

/// Values full of shell-hostile characters survive a round trip: what the env
/// destination writes, the env source reads back to the identical value (the
/// canonical lines of two generations match exactly).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_env_destination_round_trips_hostile_values() {
    let h = Harness::new().await;
    let hostile = "a\"quote b\\slash $dollar `tick\nnewline\ttab";
    h.cmd()
        .args(["store", "set", "NASTY"])
        .write_stdin(format!("{hostile}\n"))
        .assert()
        .success();

    h.cmd()
        .args([
            "sync",
            "--from",
            "local",
            "--to",
            "env:gen1.env",
            "--secret",
            "NASTY",
        ])
        .assert()
        .success();
    h.cmd()
        .args([
            "sync",
            "--from",
            "env:gen1.env",
            "--to",
            "env:gen2.env",
            "--secret",
            "NASTY",
        ])
        .assert()
        .success();

    let gen1 = h.read("gen1.env");
    let gen2 = h.read("gen2.env");
    assert_eq!(gen1, gen2, "format -> parse -> format must be stable");
    // Spot-check the escapes actually exercised the quoting rules.
    assert!(gen1.contains("\\\"") && gen1.contains("\\$") && gen1.contains("\\n"));
}

/// Invalid store specs and secret specs fail through the binary with the
/// guidance the parser promises.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_invalid_spec_errors_name_the_problem() {
    let h = Harness::new().await;
    h.write("source.env", "FOO=1\n");

    h.cmd()
        .args(["sync", "--from", "env:source.env", "--to", "bitwarden"])
        .assert()
        .failure()
        .stderr(contains("not yet supported"));

    h.cmd()
        .args([
            "sync",
            "--from",
            "env:source.env",
            "--to",
            "github:just-a-name",
        ])
        .assert()
        .failure()
        .stderr(contains("github:<owner>/<repo>"));

    h.cmd()
        .args(["sync", "--from", "nope", "--to", "env:out.env"])
        .assert()
        .failure()
        .stderr(contains("invalid source 'nope'"));

    h.cmd()
        .args([
            "sync",
            "--from",
            "env:source.env",
            "--to",
            "env:out.env",
            "--secret",
            "=item",
        ])
        .assert()
        .failure()
        .stderr(contains("name is empty"));

    // `store set` from a pipe with nothing on stdin is an error, not an empty
    // secret.
    h.cmd()
        .args(["store", "set", "FOO"])
        .write_stdin("")
        .assert()
        .failure()
        .stderr(contains("no value provided on stdin"));
}

/// A broken config file is an error that names the file — malformed JSON and
/// an unknown store type both fail at the trust boundary.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_malformed_config_errors_name_the_file() {
    let h = Harness::new().await;
    h.write("gh-secrets.json", "{ not json");
    h.cmd()
        .args(["sync"])
        .assert()
        .failure()
        .stderr(contains("gh-secrets.json"));

    h.write(
        "gh-secrets.json",
        &serde_json::to_string_pretty(&json!({
            "source": {"type": "carrier-pigeon"},
            "secrets": [{"name": "FOO"}],
            "destinations": [],
        }))
        .unwrap(),
    );
    h.cmd()
        .args(["list"])
        .assert()
        .failure()
        .stderr(contains("gh-secrets.json"))
        .stderr(contains("carrier-pigeon"));
}

/// `list` renders the Bitwarden mapping — including the config's
/// `default_field` and per-secret `field` overrides — without contacting
/// Bitwarden or needing any credential.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_list_renders_bitwarden_mapping_with_default_field() {
    let h = Harness::new().await;
    h.write(
        "gh-secrets.json",
        &serde_json::to_string_pretty(&json!({
            "source": {"type": "bitwarden", "default_field": "fields.TOKEN"},
            "secrets": [
                {"name": "PLAIN"},
                {"name": "NOTEY", "item": "shared item", "field": "notes"},
            ],
            "destinations": [{"type": "github", "repository": "owner/repo1"}],
        }))
        .unwrap(),
    );
    h.cmd()
        .args(["list"])
        .assert()
        .success()
        .stdout(contains("source: bitwarden"))
        .stdout(contains(
            "PLAIN  (bitwarden item 'PLAIN', field 'fields.TOKEN')",
        ))
        .stdout(contains(
            "NOTEY  (bitwarden item 'shared item', field 'notes')",
        ));

    // Without a default_field the implicit `password` shows.
    h.write(
        "gh-secrets.json",
        &serde_json::to_string_pretty(&json!({
            "source": {"type": "bitwarden"},
            "secrets": [{"name": "PLAIN"}],
            "destinations": [],
        }))
        .unwrap(),
    );
    h.cmd().args(["list"]).assert().success().stdout(contains(
        "PLAIN  (bitwarden item 'PLAIN', field 'password')",
    ));
}

/// `check` works with a pure-argument pipeline too (no config file at all).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_check_with_pure_args() {
    let h = Harness::new().await;
    h.write("source.env", "FOO=1\n");
    let args = [
        "check",
        "--from",
        "env:source.env",
        "--to",
        "env:out.env",
        "--secret",
        "FOO",
    ];
    h.cmd()
        .args(args)
        .assert()
        .success()
        .stdout(contains("check (config: arguments):"))
        .stdout(contains("1 to push (FOO)"));
    // Check writes nothing — not even state — so it stays pending.
    assert!(!h.dir.path().join(".gh-secrets-state.json").exists());
    h.cmd()
        .args(args)
        .assert()
        .success()
        .stdout(contains("1 to push (FOO)"));
}
