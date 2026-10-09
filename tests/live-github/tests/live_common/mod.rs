//! Helpers for the live e2e suite (`tests/e2e_live.rs`) and its loopback proof
//! (`tests/sandbox_fixture.rs`). `mod live_common` in a sibling integration test
//! pulls these in; not every consumer uses every item.
//!
//! Gated by `GH_SECRETS_LIVE_TEST=1`. When the env var is unset, callers must
//! early-return so the default gate still compiles and runs the binary as
//! cheap no-ops. When set, these helpers:
//!
//! - Read the GitHub token from `GH_TOKEN`.
//! - Read the sandbox repo's `owner/name` from `GH_SECRETS_E2E_SANDBOX_REPO`
//!   (an Actions secret in CI, an environment variable locally). It is
//!   configuration, never a literal in the tree; unset or malformed, every
//!   sandbox test fails naming the key rather than skipping.
//! - Confirm once per process that the sandbox repo is reachable.
//! - Hand each test a unique secret-name prefix, so parallel tests against the
//!   shared sandbox can never collide on the GitHub side.
//! - In `Drop`, delete every secret the test created (best-effort).
//!
//! The API base is `https://api.github.com` unless `GH_SECRETS_LIVE_API_BASE`
//! points the helpers *and* the spawned binary at a double instead; that
//! override exists only for `tests/sandbox_fixture.rs` and local proofs.
//!
//! The sandbox repo is intentionally left in place between runs; secrets that
//! survive a panicked run are easy to spot — they share the `E2E_` prefix.
#![allow(dead_code)]

pub mod journeys;

use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

pub const LIVE_ENV: &str = "GH_SECRETS_LIVE_TEST";
pub const TOKEN_ENV: &str = "GH_TOKEN";
/// The configuration key naming the sandbox repo (`owner/name`).
pub const SANDBOX_REPO_ENV: &str = "GH_SECRETS_E2E_SANDBOX_REPO";
/// Test-only API base override (helpers and spawned binary alike).
pub const API_BASE_ENV: &str = "GH_SECRETS_LIVE_API_BASE";
pub const GITHUB_API_BASE: &str = "https://api.github.com";

// llmlint: ignore[names_match_behavior] this is exactly the GH_SECRETS_LIVE_TEST gate; the token half is token()'s, which every live test calls next and which fails with the variable to set.
pub fn live_enabled() -> bool {
    env::var(LIVE_ENV).as_deref() == Ok("1")
}

pub fn token() -> String {
    env::var(TOKEN_ENV)
        .ok()
        .filter(|t| !t.trim().is_empty())
        .expect("GH_TOKEN must be set to a non-empty token when GH_SECRETS_LIVE_TEST=1")
}

/// The sandbox repo's `owner/name`, read from [`SANDBOX_REPO_ENV`]. Panics
/// naming the key when it is unset, empty or not a plain `owner/name`.
pub fn sandbox_repo() -> String {
    let v = env::var(SANDBOX_REPO_ENV).unwrap_or_default();
    let v = v.trim();
    // Each half is a GitHub owner or repo name: ASCII letters, digits, `.`, `-`
    // and `_`, never a dot segment — so nothing in the value can reshape the
    // request URLs it is interpolated into.
    let component = |c: &str| {
        !c.is_empty()
            && c != "."
            && c != ".."
            && c.chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
    };
    let valid =
        matches!(v.split_once('/'), Some((owner, name)) if component(owner) && component(name));
    assert!(
        valid,
        "{SANDBOX_REPO_ENV} must be set to the sandbox repo as owner/name when {LIVE_ENV}=1 \
         (an Actions secret of that name in CI, an environment variable locally); \
         got {}",
        if v.is_empty() {
            "nothing"
        } else {
            "a value that is not owner/name"
        }
    );
    v.to_string()
}

/// `GH_SECRETS_LIVE_API_BASE` when set (a double), else the real GitHub API.
/// The override carries the token, so it must be a plain-HTTP loopback
/// address (where a test double listens); anything else panics naming it.
pub fn api_base() -> String {
    let Some(base) = env::var(API_BASE_ENV)
        .ok()
        .map(|b| b.trim().trim_end_matches('/').to_string())
        .filter(|b| !b.is_empty())
    else {
        return GITHUB_API_BASE.to_string();
    };
    let loopback = ["http://127.0.0.1:", "http://localhost:", "http://[::1]:"]
        .iter()
        .any(|p| {
            base.strip_prefix(p)
                .is_some_and(|port| !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()))
        });
    assert!(
        loopback,
        "{API_BASE_ENV} must be a loopback double's http://127.0.0.1:<port> (or localhost / [::1]); \
         unset it to target the real GitHub API"
    );
    base
}

/// Whether the suite targets the real GitHub API (no double configured).
pub fn targets_real_github() -> bool {
    api_base() == GITHUB_API_BASE
}

/// Check once per process that the configured sandbox repo is reachable with
/// the token and is the private repo it names, failing with the key to fix
/// when it is not.
pub fn ensure_sandbox_repo(repo: &str) {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let v = http_get(&format!("/repos/{repo}")).unwrap_or_else(|e| {
            panic!(
                "the sandbox repo named by {SANDBOX_REPO_ENV} is not reachable with {TOKEN_ENV} \
                 ({e}); create it (private) on the token's account or fix {SANDBOX_REPO_ENV}"
            )
        });
        let same_repo = v["full_name"]
            .as_str()
            .is_some_and(|n| n.eq_ignore_ascii_case(repo));
        assert!(
            same_repo && v["private"] == true,
            "the repo named by {SANDBOX_REPO_ENV} must be that private repo itself \
             (got full_name {}, private {}); make it private or fix {SANDBOX_REPO_ENV}",
            v["full_name"],
            v["private"]
        );
    });
}

/// One test's worth of state: a tempdir for `GH_SECRETS_HOME`, a unique secret
/// prefix, and the sandbox repo slug. `Drop` deletes any secret matching the
/// prefix that survives the test.
pub struct LiveSession {
    pub home: TempDir,
    /// Working directory for the session's files (source env files); the
    /// binary runs with this as CWD so relative paths and state stay isolated
    /// from the repository checkout.
    pub dir: TempDir,
    pub prefix: String,
    pub repo: String,
}

impl LiveSession {
    pub fn new(test_name: &str) -> Self {
        assert!(
            live_enabled(),
            "LiveSession::new called without {LIVE_ENV}=1; tests must early-return first"
        );
        let repo = sandbox_repo();
        ensure_sandbox_repo(&repo);
        let home = TempDir::new().expect("tempdir");
        let dir = TempDir::new().expect("tempdir");
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let prefix = format!("E2E_{}_{nanos}_{n}", sanitize(test_name));
        Self {
            home,
            dir,
            prefix,
            repo,
        }
    }

    /// A fresh `gh-secrets` command pre-wired to this session's tempdir. The
    /// binary talks to the same API base as the helpers: the real GitHub API
    /// (any `GH_SECRETS_API_BASE` from the parent shell is cleared) unless
    /// `GH_SECRETS_LIVE_API_BASE` names a double.
    pub fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("gh-secrets").expect("locate gh-secrets bin");
        c.current_dir(self.dir.path());
        c.env("GH_SECRETS_HOME", self.home.path());
        if targets_real_github() {
            c.env_remove("GH_SECRETS_API_BASE");
        } else {
            c.env("GH_SECRETS_API_BASE", api_base());
        }
        c
    }

    /// Write (or overwrite) this session's source env file with the given
    /// `NAME=value` entries, returning the `--from` spec for it.
    pub fn write_source_env(&self, entries: &[(&str, &str)]) -> String {
        let body: String = entries.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
        std::fs::write(self.dir.path().join("source.env"), body).expect("write source.env");
        "env:source.env".to_string()
    }

    /// Build a secret name unique to this session.
    pub fn secret_name(&self, leaf: &str) -> String {
        format!("{}_{}", self.prefix, sanitize(leaf))
    }

    /// List the names of every secret currently on the sandbox repo.
    // llmlint: ignore-block[names_match_behavior, boundary_inputs_validated] the sandbox repo only ever holds this suite's per-test-prefixed secrets, which Drop deletes, so one 100-item page is all of them; the empty-on-error fallback serves the Drop cleanup below, which must never panic while unwinding, and asserting callers fail on the missing name anyway.
    pub fn remote_secret_names(&self) -> Vec<String> {
        let path = format!("/repos/{}/actions/secrets?per_page=100", self.repo);
        let v: Value = match http_get(&path) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        v["secrets"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| s["name"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }
    // llmlint: ignore-end[names_match_behavior, boundary_inputs_validated]

    /// Fetch a single secret's metadata (`name`, `created_at`, `updated_at`).
    /// Returns `None` if the secret does not exist.
    // llmlint: ignore[names_match_behavior] every caller asserts on the secret it just synced, so any lookup failure surfaces as that assertion failing with the secret named; distinguishing auth or transport errors here would change no outcome of the live suite.
    pub fn remote_secret(&self, name: &str) -> Option<Value> {
        let path = format!("/repos/{}/actions/secrets/{name}", self.repo);
        http_get(&path).ok()
    }
}

impl Drop for LiveSession {
    fn drop(&mut self) {
        if !live_enabled() {
            return;
        }
        let names = self.remote_secret_names();
        let prefix = self.prefix.clone();
        for name in names.iter().filter(|n| n.starts_with(&prefix)) {
            let path = format!("/repos/{}/actions/secrets/{name}", self.repo);
            let _ = http_delete(&path);
        }
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .user_agent("gh-secrets-e2e")
        .build()
        .expect("build http client")
}

pub fn http_get(path: &str) -> Result<Value, String> {
    let url = format!("{}{path}", api_base());
    let resp = client()
        .get(url)
        .bearer_auth(token())
        .header("accept", "application/vnd.github+json")
        .send()
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    let body = resp.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("GET {path}: {status}: {body}"));
    }
    serde_json::from_str(&body).map_err(|e| e.to_string())
}

pub fn http_delete(path: &str) -> Result<(), String> {
    let url = format!("{}{path}", api_base());
    let resp = client()
        .delete(url)
        .bearer_auth(token())
        .header("accept", "application/vnd.github+json")
        .send()
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("DELETE {path}: {}", resp.status()));
    }
    Ok(())
}
