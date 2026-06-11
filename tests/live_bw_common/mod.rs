//! Helpers for the live Bitwarden e2e suite (`tests/e2e_live_bitwarden.rs`).
//!
//! These drive `gh-secrets` against a **real** Bitwarden account — an isolated
//! account that exists only for this test (so seeding and deleting items in it
//! is safe). They are gated twice: `GH_SECRETS_LIVE_TEST=1` *and* the isolated
//! account's API-key credentials present in the environment. When either is
//! missing, callers must early-return (via `skip_if_no_bw_live!`) so the
//! default gate still compiles and runs the binary as cheap no-ops.
//!
//! ## Isolation from the developer's own Bitwarden
//!
//! The `bw` CLI keeps all of its state (the logged-in account, the encrypted
//! vault cache, the session) in a single app-data directory. A developer
//! running this suite is almost certainly already logged in to their *own*
//! Bitwarden account in the default location. So every session here points
//! `BITWARDENCLI_APPDATA_DIR` at its own tempdir: `bw login --apikey` against
//! the isolated account lands in that throwaway dir and never disturbs (or is
//! confused by) the developer's real login. Crucially, `gh-secrets` spawns
//! `bw` as a child that inherits this env var, so pointing the *gh-secrets*
//! process at the tempdir flows all the way down to the `bw` it shells out to.
//!
//! ## Why each test logs in independently
//!
//! nextest runs each `#[test]` in its own process, so there is no in-process
//! state to share — a `OnceLock` login would re-run once per test anyway.
//! Giving every test its own app-data tempdir keeps them from racing on `bw`'s
//! state files, so they remain safe to run in parallel; isolation on the
//! Bitwarden side comes from a per-test item-name prefix plus `Drop` cleanup.

use std::process::Command as StdCommand;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use assert_cmd::Command;
use serde_json::{json, Value};
use tempfile::TempDir;

pub const LIVE_ENV: &str = "GH_SECRETS_LIVE_TEST";

/// Env vars the isolated account's API-key credentials are read from. These are
/// deliberately distinct from the canonical `BW_*`/`BITWARDEN_*` names so a
/// developer's own logged-in Bitwarden credentials can never be mistaken for
/// the throwaway test account's.
pub const CLIENT_ID_ENV: &str = "GH_SECRETS_BW_E2E_CLIENT_ID";
pub const CLIENT_SECRET_ENV: &str = "GH_SECRETS_BW_E2E_CLIENT_SECRET";
pub const PASSWORD_ENV: &str = "GH_SECRETS_BW_E2E_PASSWORD";

pub fn live_enabled() -> bool {
    std::env::var(LIVE_ENV).as_deref() == Ok("1")
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// The isolated account's resolved API-key credentials, or `None` if any are
/// missing (in which case the suite skips).
pub fn creds() -> Option<BwCreds> {
    Some(BwCreds {
        client_id: env_nonempty(CLIENT_ID_ENV)?,
        client_secret: env_nonempty(CLIENT_SECRET_ENV)?,
        password: env_nonempty(PASSWORD_ENV)?,
    })
}

/// True only when both gates are satisfied: live mode on *and* the isolated
/// account's credentials available.
pub fn bw_live_enabled() -> bool {
    live_enabled() && creds().is_some()
}

#[macro_export]
macro_rules! skip_if_no_bw_live {
    () => {
        if !$crate::live_bw_common::bw_live_enabled() {
            eprintln!(
                "skip: live bitwarden test (set {}=1 + {}/{}/{} to run)",
                $crate::live_bw_common::LIVE_ENV,
                $crate::live_bw_common::CLIENT_ID_ENV,
                $crate::live_bw_common::CLIENT_SECRET_ENV,
                $crate::live_bw_common::PASSWORD_ENV,
            );
            return;
        }
    };
}

#[derive(Clone)]
pub struct BwCreds {
    pub client_id: String,
    pub client_secret: String,
    pub password: String,
}

/// One test's worth of state against the isolated Bitwarden account:
///
/// - `home` — an isolated `GH_SECRETS_HOME` config root.
/// - `dir` — a working directory for the run's env-file destinations + state.
/// - `appdata` — the throwaway `bw` app-data dir this session logs in to.
/// - `prefix` — a unique item-name prefix so parallel tests never collide.
/// - `session` — an unlocked `bw` session for the isolated vault, used to seed
///   and tear down items directly (the product CLI cannot write to Bitwarden).
pub struct BwLiveSession {
    pub home: TempDir,
    pub dir: TempDir,
    pub appdata: TempDir,
    pub prefix: String,
    creds: BwCreds,
    session: String,
    created_ids: Mutex<Vec<String>>,
}

impl BwLiveSession {
    /// Log in to the isolated account in a fresh app-data dir and unlock it,
    /// leaving a ready-to-seed session. Panics (failing the test loudly) if any
    /// step fails — these run only when `bw_live_enabled()`.
    pub fn new(test_name: &str) -> Self {
        assert!(
            bw_live_enabled(),
            "BwLiveSession::new called while disabled; tests must skip first"
        );
        let creds = creds().expect("creds present (checked by bw_live_enabled)");
        let home = TempDir::new().expect("tempdir");
        let dir = TempDir::new().expect("tempdir");
        let appdata = TempDir::new().expect("tempdir");

        // Cold login + unlock against the isolated account in our own app-data
        // dir, exercising the exact `bw` commands the product CLI runs.
        bw_checked(
            &appdata,
            &["login", "--apikey"],
            &[
                ("BW_CLIENTID", &creds.client_id),
                ("BW_CLIENTSECRET", &creds.client_secret),
            ],
        );
        let session = bw_checked(
            &appdata,
            &["unlock", "--raw", "--passwordenv", "BW_PASSWORD"],
            &[("BW_PASSWORD", &creds.password)],
        )
        .trim()
        .to_string();
        assert!(!session.is_empty(), "bw unlock returned an empty session");
        bw_checked(&appdata, &["sync", "--session", &session], &[]);

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let prefix = format!("ghse2e_{}_{nanos}_{n}", sanitize(test_name));

        Self {
            home,
            dir,
            appdata,
            prefix,
            creds,
            session,
            created_ids: Mutex::new(Vec::new()),
        }
    }

    /// A `gh-secrets` command pointed at this session's isolated config root and
    /// `bw` app-data dir, carrying the isolated account's credentials under the
    /// canonical `BW_*` names. Any inherited Bitwarden state (a developer's
    /// session/creds, or a real `GH_SECRETS_API_BASE`) is scrubbed so the run
    /// is deterministic. Because the app-data dir is already logged-in+unlocked,
    /// this exercises gh-secrets' status → unlock → sync → fetch path.
    pub fn cmd(&self) -> Command {
        self.cmd_in_appdata(self.appdata.path())
    }

    /// Like [`cmd`](Self::cmd) but points `bw` at an arbitrary app-data dir.
    /// Passing a *fresh* (unauthenticated) dir makes gh-secrets perform the
    /// full cold path itself: `bw login --apikey` → unlock → sync → fetch.
    pub fn cmd_in_appdata(&self, appdata: &std::path::Path) -> Command {
        let mut c = Command::cargo_bin("gh-secrets").expect("locate gh-secrets bin");
        c.current_dir(self.dir.path())
            .env("GH_SECRETS_HOME", self.home.path())
            .env("BITWARDENCLI_APPDATA_DIR", appdata)
            .env("BW_CLIENTID", &self.creds.client_id)
            .env("BW_CLIENTSECRET", &self.creds.client_secret)
            .env("BW_PASSWORD", &self.creds.password)
            .env_remove("GH_SECRETS_API_BASE")
            .env_remove("BW_SESSION")
            .env_remove("BITWARDEN_SESSION")
            .env_remove("BITWARDEN_CLIENT_ID")
            .env_remove("BITWARDEN_CLIENT_SECRET")
            .env_remove("BITWARDEN_MASTER_PASSWORD")
            .env_remove("BITWARDEN_PASSWORD");
        c
    }

    /// A unique Bitwarden item name for this session.
    pub fn item_name(&self, leaf: &str) -> String {
        format!("{}_{}", self.prefix, sanitize(leaf))
    }

    /// Seed a Login item in the isolated vault, returning its name. The item
    /// carries a username, password, notes, and one hidden custom field so the
    /// field-selector tests can pull each one back out. Records the created id
    /// for `Drop` cleanup.
    #[allow(clippy::too_many_arguments)]
    pub fn seed_login(
        &self,
        leaf: &str,
        username: &str,
        password: &str,
        notes: &str,
        custom_field: (&str, &str),
    ) -> String {
        let name = self.item_name(leaf);
        let item = json!({
            "organizationId": null,
            "collectionIds": null,
            "folderId": null,
            "type": 1, // Login
            "name": name,
            "notes": notes,
            "favorite": false,
            "fields": [{ "name": custom_field.0, "value": custom_field.1, "type": 1 }],
            "login": { "username": username, "password": password, "uris": [], "totp": null },
            "reprompt": 0,
        });
        let encoded = {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.encode(item.to_string())
        };
        let out = bw_checked(
            &self.appdata,
            &["create", "item", &encoded, "--session", &self.session],
            &[],
        );
        let created: Value = serde_json::from_str(&out).expect("parse created item JSON");
        let id = created["id"]
            .as_str()
            .expect("created item has an id")
            .to_string();
        self.created_ids.lock().unwrap().push(id);
        name
    }
}

impl Drop for BwLiveSession {
    fn drop(&mut self) {
        // Best-effort cleanup, then log out so the throwaway app-data dir leaves
        // no lingering server session.
        //
        // Re-unlock for a fresh session rather than reusing `self.session`: when
        // a test let gh-secrets unlock *this same* app-data dir, that minted a
        // new session and invalidated ours, so a delete with the stale token
        // would silently no-op. A fresh unlock (the dir is still logged in)
        // always yields a usable session. `--permanent` deletes outright instead
        // of leaving the item in the vault's trash.
        let ids = self.created_ids.lock().unwrap().clone();
        if !ids.is_empty() {
            if let Ok(out) = run_bw(
                &self.appdata,
                &["unlock", "--raw", "--passwordenv", "BW_PASSWORD"],
                &[("BW_PASSWORD", &self.creds.password)],
            ) {
                if out.status.success() {
                    let session = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    let _ = run_bw(&self.appdata, &["sync", "--session", &session], &[]);
                    for id in &ids {
                        let _ = run_bw(
                            &self.appdata,
                            &["delete", "item", id, "--permanent", "--session", &session],
                            &[],
                        );
                    }
                }
            }
        }
        let _ = run_bw(&self.appdata, &["logout"], &[]);
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Run `bw` with the given app-data dir and extra env, returning the raw
/// `Output`. The app-data dir is the isolation boundary; extra env carries
/// credentials `bw` reads natively (`BW_CLIENTID`, `BW_PASSWORD`, ...).
fn run_bw(
    appdata: &TempDir,
    args: &[&str],
    env: &[(&str, &str)],
) -> std::io::Result<std::process::Output> {
    let mut cmd = StdCommand::new("bw");
    cmd.args(args)
        .env("BITWARDENCLI_APPDATA_DIR", appdata.path());
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output()
}

/// Run `bw` and return its stdout as a `String`, panicking with stderr on
/// failure so a broken setup step fails the test with a clear message. `bw`
/// never writes a secret value to stderr, so surfacing it is safe.
fn bw_checked(appdata: &TempDir, args: &[&str], env: &[(&str, &str)]) -> String {
    let out = run_bw(appdata, args, env).unwrap_or_else(|e| {
        panic!(
            "running `bw {}`: {e} (is the Bitwarden CLI installed?)",
            args[0]
        )
    });
    assert!(
        out.status.success(),
        "`bw {}` failed: {}",
        args[0],
        String::from_utf8_lossy(&out.stderr).trim()
    );
    String::from_utf8(out.stdout).expect("bw stdout is UTF-8")
}
