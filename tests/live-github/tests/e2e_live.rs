//! Live e2e tests against the real GitHub API.
//!
//! These tests are skipped (as no-op passing tests) unless
//! `GH_SECRETS_LIVE_TEST=1` is set; then they need `GH_TOKEN` and
//! `GH_SECRETS_E2E_SANDBOX_REPO` (the sandbox repo's `owner/name`) and fail
//! naming whichever is missing. The sandbox tests share that one repo, isolate
//! themselves via a per-test unique secret-name prefix, and clean up the
//! secrets they create in `Drop`. Their bodies live in `live_common/journeys.rs`
//! so `tests/sandbox_fixture.rs` can run the same code against a double.
//!
//! Run with: `just test-live` (or directly:
//! `GH_SECRETS_LIVE_TEST=1 GH_TOKEN=... GH_SECRETS_E2E_SANDBOX_REPO=owner/name
//! cargo nextest run -p gh-secrets-live-github` after `cargo build -p gh-secrets`).

mod live_common;

use std::process::Command as StdCommand;

use live_common::{journeys, live_enabled, targets_real_github, token, LIVE_ENV};
use tempfile::TempDir;

macro_rules! skip_if_no_live {
    () => {
        if !live_enabled() {
            eprintln!("skip: live test (set {LIVE_ENV}=1 + GH_TOKEN to run)");
            return;
        }
    };
}

#[test]
fn live_sync_creates_secret_visible_via_api() {
    skip_if_no_live!();
    journeys::sync_creates_secret_visible_via_api();
}

#[test]
fn live_noop_resync_returns_nothing_to_do() {
    skip_if_no_live!();
    journeys::noop_resync_returns_nothing_to_do();
}

#[test]
fn live_update_value_propagates_on_resync() {
    skip_if_no_live!();
    journeys::update_value_propagates_on_resync();
}

#[test]
fn live_recovers_from_invalid_token() {
    skip_if_no_live!();
    journeys::recovers_from_invalid_token();
}

#[test]
fn live_undeclared_secret_is_not_deleted_remotely() {
    skip_if_no_live!();
    journeys::undeclared_secret_is_not_deleted_remotely();
}

/// The cross-platform install script (`scripts/install.sh`) must download the
/// real published release, verify its SHA-256 checksum, and drop a working
/// binary onto the chosen PATH. This drives the canonical `curl ... | sh`
/// experience end-to-end against GitHub: it resolves the latest release tag
/// (passing `GITHUB_TOKEN` so the API call is not rate-limited), downloads and
/// checksum-verifies the archive for this host platform, extracts it, and
/// installs the binary, which we then run to prove it is functional.
///
/// Only meaningful on the platforms the script targets with a POSIX shell;
/// `live-e2e` runs on Linux, where `sh`/`tar`/`sha256sum` are present.
#[test]
fn live_install_script_downloads_and_verifies_release() {
    skip_if_no_live!();

    let bindir = TempDir::new().expect("tempdir for install target");
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/install.sh");

    let mut cmd = StdCommand::new("sh");
    cmd.arg(script)
        .arg("--to")
        .arg(bindir.path())
        .env_remove("GITHUB_TOKEN");
    // The script reads GITHUB_TOKEN (not GH_TOKEN) to authenticate the
    // "latest release" API call and lift the unauthenticated rate limit. The
    // token belongs to the suite's API base, so it goes to GitHub only when
    // that base is GitHub (not a double's token under GH_SECRETS_LIVE_API_BASE).
    if targets_real_github() {
        cmd.env("GITHUB_TOKEN", token());
    }
    let output = cmd.output().expect("run scripts/install.sh");

    assert!(
        output.status.success(),
        "install.sh failed (status {:?}):\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // The installed binary must run. The script names it `gh-secrets` on every
    // platform the live job exercises (no `.exe` on Linux/macOS).
    let installed = bindir.path().join("gh-secrets");
    assert!(
        installed.is_file(),
        "expected installed binary at {}",
        installed.display()
    );
    let version = StdCommand::new(&installed)
        .arg("--version")
        .output()
        .expect("run installed gh-secrets --version");
    assert!(version.status.success(), "installed binary failed to run");
    let stdout = String::from_utf8_lossy(&version.stdout);
    assert!(
        stdout.contains("gh-secrets"),
        "unexpected --version output: {stdout}"
    );
}
