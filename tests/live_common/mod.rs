//! Helpers for the live e2e suite (`tests/e2e_live.rs`).
//!
//! Gated by `GH_SECRETS_LIVE_TEST=1`. When the env var is unset, callers must
//! early-return so the default gate still compiles and runs the binary as
//! cheap no-ops. When set, these helpers:
//!
//! - Read the GitHub token from `GH_TOKEN`.
//! - Discover the authenticated user's login once per process.
//! - Idempotently create a sandbox repo `gh-secrets-e2e-sandbox` (private).
//! - Hand each test a unique secret-name prefix, so parallel tests against the
//!   shared sandbox can never collide on the GitHub side.
//! - In `Drop`, delete every secret the test created (best-effort).
//!
//! The sandbox repo is intentionally left in place between runs; secrets that
//! survive a panicked run are easy to spot — they share the `E2E_` prefix.

use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use assert_cmd::Command;
use serde_json::{json, Value};
use tempfile::TempDir;

pub const LIVE_ENV: &str = "GH_SECRETS_LIVE_TEST";
pub const TOKEN_ENV: &str = "GH_TOKEN";
pub const SANDBOX_REPO_NAME: &str = "gh-secrets-e2e-sandbox";

pub fn live_enabled() -> bool {
    env::var(LIVE_ENV).as_deref() == Ok("1")
}

pub fn token() -> String {
    env::var(TOKEN_ENV).expect("GH_TOKEN must be set when GH_SECRETS_LIVE_TEST=1")
}

pub fn owner() -> &'static str {
    static OWNER: OnceLock<String> = OnceLock::new();
    OWNER.get_or_init(|| {
        let v: Value = http_get("/user").expect("GET /user for live tests");
        v["login"]
            .as_str()
            .expect("user.login is a string")
            .to_string()
    })
}

pub fn repo_slug() -> String {
    format!("{}/{}", owner(), SANDBOX_REPO_NAME)
}

/// Create the sandbox repo if it doesn't already exist. Runs once per process.
pub fn ensure_sandbox_repo() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let body = json!({
            "name": SANDBOX_REPO_NAME,
            "private": true,
            "description": "Sandbox for gh-secrets e2e tests; safe to delete.",
            "has_issues": false,
            "has_projects": false,
            "has_wiki": false,
            "auto_init": true,
        });
        let resp = client()
            .post("https://api.github.com/user/repos")
            .bearer_auth(token())
            .header("accept", "application/vnd.github+json")
            .json(&body)
            .send()
            .expect("POST /user/repos");
        let status = resp.status().as_u16();
        // 201 Created, 202 Accepted, or 422 (already exists / name taken).
        if status != 201 && status != 202 && status != 422 {
            let body = resp.text().unwrap_or_default();
            panic!("creating sandbox repo failed: HTTP {status}: {body}");
        }
    });
}

/// One test's worth of state: a tempdir for `GH_SECRETS_HOME`, a unique secret
/// prefix, and the sandbox repo slug. `Drop` deletes any secret matching the
/// prefix that survives the test.
pub struct LiveSession {
    pub home: TempDir,
    pub prefix: String,
    pub repo: String,
}

impl LiveSession {
    pub fn new(test_name: &str) -> Self {
        assert!(
            live_enabled(),
            "LiveSession::new called without {LIVE_ENV}=1; tests must early-return first"
        );
        ensure_sandbox_repo();
        let home = TempDir::new().expect("tempdir");
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let prefix = format!("E2E_{}_{nanos}_{n}", sanitize(test_name));
        Self {
            home,
            prefix,
            repo: repo_slug(),
        }
    }

    /// A fresh `gh-secrets` command pre-wired to this session's tempdir. The
    /// real GitHub API base is used (not a mock), so any `GH_SECRETS_API_BASE`
    /// from the parent shell is explicitly cleared.
    pub fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("gh-secrets").expect("locate gh-secrets bin");
        c.env("GH_SECRETS_HOME", self.home.path());
        c.env_remove("GH_SECRETS_API_BASE");
        c
    }

    /// Build a secret name unique to this session.
    pub fn secret_name(&self, leaf: &str) -> String {
        format!("{}_{}", self.prefix, sanitize(leaf))
    }

    /// List the names of every secret currently on the sandbox repo.
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

    /// Fetch a single secret's metadata (`name`, `created_at`, `updated_at`).
    /// Returns `None` if the secret does not exist.
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
    let url = format!("https://api.github.com{path}");
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
    let url = format!("https://api.github.com{path}");
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
