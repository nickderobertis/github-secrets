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
