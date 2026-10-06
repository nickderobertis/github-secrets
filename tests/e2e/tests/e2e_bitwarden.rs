//! Offline end-to-end tests for the Bitwarden source.
//!
//! Drives the compiled `gh-secrets` binary with a stand-in `bw` first on `PATH`
//! (`support/fake_bw.rs`, built by this crate), so the real subprocess contract
//! is exercised on every gate run: the argv `gh-secrets` passes, the `BW_*` env
//! handoff, and how it treats `bw`'s exit codes and output. The live suite
//! (`tests/live-bitwarden`) proves the same pipeline against the real service;
//! this one proves it deterministically, with no account or network.
//!
//! What's covered:
//! - The cold path (unauthenticated → api-key login → unlock → sync → get item)
//!   pulling every field selector into an env-file destination, then a no-op
//!   re-sync.
//! - An already-logged-in (locked) vault skips login; a preset `BW_SESSION`
//!   skips status/login/unlock entirely.
//! - Credentials stored with `gh-secrets auth bitwarden` reach `bw` when the
//!   environment has none.
//! - `source list` over a Bitwarden source, unscoped and scoped by the
//!   config's collection / organization.
//! - Failure edges: wrong master password, missing client id, item not found,
//!   no `bw` on PATH, malformed `bw status` output, and an empty unlock token —
//!   each a precise error that names `bw`, never a secret value, with nothing
//!   written.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use serde_json::{json, Value};
use tempfile::TempDir;

const SESSION: &str = "fake-session-token";
const CLIENT_ID: &str = "user.client-id";
const CLIENT_SECRET: &str = "client-secret-value";
const MASTER_PASSWORD: &str = "correct horse battery staple";
const PASSPHRASE: &str = "e2e-bitwarden-passphrase";

/// Every Bitwarden credential variable `gh-secrets` reads. Cleared on each
/// command so a developer's real login can never leak into (or be used by) a test.
const BW_VARS: &[&str] = &[
    "BW_CLIENTID",
    "BW_CLIENTSECRET",
    "BW_PASSWORD",
    "BW_SESSION",
    "BITWARDEN_CLIENT_ID",
    "BITWARDEN_CLIENT_SECRET",
    "BITWARDEN_MASTER_PASSWORD",
    "BITWARDEN_PASSWORD",
    "BITWARDEN_SESSION",
];

struct Harness {
    dir: TempDir,
}

impl Harness {
    fn new(status: &str) -> Self {
        let h = Self {
            dir: TempDir::new().expect("tempdir"),
        };
        h.write_state(json!({
            "status": status,
            "client_id": CLIENT_ID,
            "client_secret": CLIENT_SECRET,
            "password": MASTER_PASSWORD,
            "session": SESSION,
            "items": [
                {
                    "id": "id-api",
                    "name": "api-item",
                    "organizationId": "org-1",
                    "collectionIds": ["col-1"],
                    "notes": "api-notes-value",
                    "fields": [{ "name": "API_URL", "value": "https://api.example", "type": 1 }],
                    "login": { "username": "api-user-value", "password": "api-password-value" }
                },
                {
                    "id": "id-other",
                    "name": "other-item",
                    "organizationId": null,
                    "collectionIds": [],
                    "notes": null,
                    "login": { "username": "other-user-value", "password": "other-password-value" }
                }
            ]
        }));
        let bin = h.dir.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let exe = format!("bw{}", std::env::consts::EXE_SUFFIX);
        fs::copy(env!("CARGO_BIN_EXE_fake-bw"), bin.join(exe)).expect("install fake bw");
        h
    }

    fn state_path(&self) -> PathBuf {
        self.dir.path().join("bw-state.json")
    }

    fn log_path(&self) -> PathBuf {
        self.dir.path().join("bw.log")
    }

    fn write_state(&self, state: Value) {
        fs::write(self.state_path(), state.to_string()).unwrap();
    }

    fn patch_state(&self, key: &str, value: Value) {
        let mut state: Value =
            serde_json::from_str(&fs::read_to_string(self.state_path()).unwrap()).unwrap();
        state[key] = value;
        self.write_state(state);
    }

    /// The `bw` invocations so far, one argv per entry.
    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.log_path())
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    fn path_with_fake_bw(&self) -> OsString {
        let mut paths = vec![self.dir.path().join("bin")];
        if let Some(existing) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&existing));
        }
        std::env::join_paths(paths).unwrap()
    }

    /// The binary with an isolated config root, the fake `bw` first on PATH,
    /// and no inherited Bitwarden credentials.
    fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("gh-secrets").expect("locate gh-secrets bin");
        c.current_dir(self.dir.path())
            .env("GH_SECRETS_HOME", self.dir.path().join("home"))
            .env("GH_SECRETS_PASSPHRASE", PASSPHRASE)
            .env("FAKE_BW_STATE", self.state_path())
            .env("FAKE_BW_LOG", self.log_path())
            .env("PATH", self.path_with_fake_bw());
        for var in BW_VARS {
            c.env_remove(var);
        }
        c
    }

    /// `cmd()` plus the three api-key credentials in the environment.
    fn cmd_with_creds(&self) -> Command {
        let mut c = self.cmd();
        c.env("BW_CLIENTID", CLIENT_ID)
            .env("BW_CLIENTSECRET", CLIENT_SECRET)
            .env("BW_PASSWORD", MASTER_PASSWORD);
        c
    }

    fn out(&self) -> PathBuf {
        self.dir.path().join("out.env")
    }
}

fn sync_args(extra: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = ["sync", "--from", "bitwarden", "--to", "env:out.env"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    args.extend(extra.iter().map(|s| s.to_string()));
    args
}

fn assert_no_value_leaks(output: &std::process::Output) {
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for value in [
        "api-password-value",
        "api-user-value",
        "api-notes-value",
        MASTER_PASSWORD,
        CLIENT_SECRET,
    ] {
        assert!(!all.contains(value), "output leaked {value:?}: {all}");
    }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

#[test]
fn e2e_bitwarden_cold_path_pulls_every_field_selector_then_noops() {
    let h = Harness::new("unauthenticated");
    let output = h
        .cmd_with_creds()
        .args(sync_args(&[
            "--secret",
            "API_PASSWORD=api-item",
            "--secret",
            "API_USER=id-api#username",
            "--secret",
            "API_NOTES=api-item#notes",
            "--secret",
            "API_URL=api-item#fields.API_URL",
        ]))
        .assert()
        .success()
        .stdout(contains("4 created"))
        .get_output()
        .clone();
    assert_no_value_leaks(&output);

    let env = read(&h.out());
    assert!(env.contains("API_PASSWORD=\"api-password-value\""), "{env}");
    assert!(env.contains("API_USER=\"api-user-value\""), "{env}");
    assert!(env.contains("API_NOTES=\"api-notes-value\""), "{env}");
    assert!(env.contains("API_URL=\"https://api.example\""), "{env}");
    assert_eq!(
        h.calls()[..4],
        [
            "status",
            "login --apikey",
            "unlock --raw --passwordenv BW_PASSWORD",
            "sync"
        ]
    );
    assert!(h.calls().contains(&"get item id-api".to_string()));

    // The fake now reports a logged-in (locked) vault, so the re-run unlocks
    // without logging in again, and finds nothing to push.
    h.cmd_with_creds()
        .args(sync_args(&[
            "--secret",
            "API_PASSWORD=api-item",
            "--secret",
            "API_USER=id-api#username",
            "--secret",
            "API_NOTES=api-item#notes",
            "--secret",
            "API_URL=api-item#fields.API_URL",
        ]))
        .assert()
        .success()
        .stdout(contains("sync: nothing to do"));
    let logins = h.calls().iter().filter(|c| c.starts_with("login")).count();
    assert_eq!(logins, 1, "a locked vault must not log in again");
}

#[test]
fn e2e_bitwarden_default_field_and_preset_session_skip_unlock() {
    let h = Harness::new("unauthenticated");
    h.cmd()
        .env("BW_SESSION", SESSION)
        .args(sync_args(&[
            "--default-field",
            "username",
            "--secret",
            "API_USER=api-item",
            "--secret",
            "OTHER_PASSWORD=other-item#password",
        ]))
        .assert()
        .success();
    let env = read(&h.out());
    assert!(env.contains("API_USER=\"api-user-value\""), "{env}");
    assert!(
        env.contains("OTHER_PASSWORD=\"other-password-value\""),
        "{env}"
    );
    assert!(
        h.calls().iter().all(|c| c.starts_with("get item")),
        "a preset session must go straight to lookups: {:?}",
        h.calls()
    );
}

#[test]
fn e2e_bitwarden_stored_credentials_reach_bw() {
    let h = Harness::new("unauthenticated");
    h.cmd()
        .args([
            "auth",
            "bitwarden",
            "--client-id",
            CLIENT_ID,
            "--client-secret",
            CLIENT_SECRET,
            "--master-password",
            MASTER_PASSWORD,
        ])
        .assert()
        .success();
    h.cmd()
        .args(sync_args(&["--secret", "API_PASSWORD=api-item"]))
        .assert()
        .success()
        .stdout(contains("1 created"));
    assert!(read(&h.out()).contains("API_PASSWORD=\"api-password-value\""));
    assert!(h.calls().contains(&"login --apikey".to_string()));
}

#[test]
fn e2e_bitwarden_source_list_honours_collection_and_organization_scope() {
    let h = Harness::new("locked");
    h.cmd_with_creds()
        .args(["source", "list", "--from", "bitwarden"])
        .assert()
        .success()
        .stdout(contains("api-item  (id-api)").and(contains("other-item  (id-other)")));

    // Scoping comes from the config's source block; `source list` reads it.
    let scoped = |collection: &str| {
        fs::write(
            h.dir.path().join("gh-secrets.json"),
            json!({
                "source": {
                    "type": "bitwarden",
                    "collection_id": collection,
                    "organization_id": "org-1"
                },
                "destinations": [],
                "secrets": []
            })
            .to_string(),
        )
        .unwrap();
    };
    scoped("col-1");
    h.cmd_with_creds()
        .args(["source", "list"])
        .assert()
        .success()
        .stdout(contains("api-item").and(contains("other-item").not()));
    assert!(h
        .calls()
        .contains(&"list items --collectionid col-1 --organizationid org-1".to_string()));

    scoped("no-such-collection");
    h.cmd_with_creds()
        .args(["source", "list"])
        .assert()
        .success()
        .stdout(contains("source: no items available"));
}

#[test]
fn e2e_bitwarden_config_driven_check_then_sync() {
    let h = Harness::new("locked");
    fs::write(
        h.dir.path().join("gh-secrets.json"),
        json!({
            "source": {"type": "bitwarden", "default_field": "password"},
            "destinations": [{"type": "env_file", "path": "out.env"}],
            "secrets": [{"name": "API_PASSWORD", "item": "api-item"}]
        })
        .to_string(),
    )
    .unwrap();
    h.cmd_with_creds()
        .arg("check")
        .assert()
        .success()
        .stdout(contains("API_PASSWORD"));
    assert!(!h.out().exists(), "check must write nothing");
    h.cmd_with_creds().arg("sync").assert().success();
    assert!(read(&h.out()).contains("API_PASSWORD=\"api-password-value\""));
}

#[test]
fn e2e_bitwarden_wrong_master_password_is_precise_and_writes_nothing() {
    let h = Harness::new("locked");
    let output = h
        .cmd_with_creds()
        .env("BW_PASSWORD", "not-the-password")
        .args(sync_args(&["--secret", "API_PASSWORD=api-item"]))
        .assert()
        .failure()
        .stderr(contains("`bw unlock` failed").and(contains("Invalid master password")))
        .get_output()
        .clone();
    assert!(!String::from_utf8_lossy(&output.stderr).contains("not-the-password"));
    assert!(!h.out().exists());
}

#[test]
fn e2e_bitwarden_missing_login_material_names_the_variable() {
    let h = Harness::new("unauthenticated");
    h.cmd()
        .env("BW_PASSWORD", MASTER_PASSWORD)
        .args(sync_args(&["--secret", "API_PASSWORD=api-item"]))
        .assert()
        .failure()
        .stderr(contains("no Bitwarden client id").and(contains("BW_CLIENTID")));

    h.cmd()
        .env("BW_CLIENTID", CLIENT_ID)
        .env("BW_PASSWORD", MASTER_PASSWORD)
        .args(sync_args(&["--secret", "API_PASSWORD=api-item"]))
        .assert()
        .failure()
        .stderr(contains("no Bitwarden client secret"));

    let locked = Harness::new("locked");
    locked
        .cmd()
        .env("BW_CLIENTID", CLIENT_ID)
        .args(sync_args(&["--secret", "API_PASSWORD=api-item"]))
        .assert()
        .failure()
        .stderr(contains("no Bitwarden master password"));
    assert!(!h.out().exists() && !locked.out().exists());
}

#[test]
fn e2e_bitwarden_wrong_api_key_surfaces_bw_login_error() {
    let h = Harness::new("unauthenticated");
    h.cmd_with_creds()
        .env("BW_CLIENTSECRET", "wrong-secret")
        .args(sync_args(&["--secret", "API_PASSWORD=api-item"]))
        .assert()
        .failure()
        .stderr(contains("`bw login --apikey` failed").and(contains("wrong-secret").not()));
}

#[test]
fn e2e_bitwarden_missing_item_and_field_name_the_secret() {
    let h = Harness::new("locked");
    h.cmd_with_creds()
        .args(sync_args(&["--secret", "GONE=no-such-item"]))
        .assert()
        .failure()
        .stderr(contains("fetching 'GONE' from Bitwarden").and(contains("Not found.")));

    h.cmd_with_creds()
        .args(sync_args(&["--secret", "NOTES=other-item#notes"]))
        .assert()
        .failure()
        .stderr(contains("extracting field 'notes'").and(contains("other-item")));
    assert!(!h.out().exists());
}

#[test]
fn e2e_bitwarden_cli_absent_or_misbehaving_is_a_precise_error() {
    let h = Harness::new("locked");
    let empty = TempDir::new().unwrap();
    h.cmd_with_creds()
        .env("PATH", empty.path())
        .args(sync_args(&["--secret", "API_PASSWORD=api-item"]))
        .assert()
        .failure()
        .stderr(contains("is the Bitwarden CLI installed?"));

    h.patch_state("status_raw", json!("this is not json"));
    h.cmd_with_creds()
        .args(sync_args(&["--secret", "API_PASSWORD=api-item"]))
        .assert()
        .failure()
        .stderr(contains("parsing `bw status` JSON"));

    h.patch_state("status_raw", Value::Null);
    h.patch_state("unlock_raw", json!("  \n"));
    h.cmd_with_creds()
        .args(sync_args(&["--secret", "API_PASSWORD=api-item"]))
        .assert()
        .failure()
        .stderr(contains("`bw unlock` returned an empty session token"));
    assert!(!h.out().exists());
}
