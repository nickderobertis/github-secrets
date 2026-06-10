//! Live e2e tests against the real GitHub API.
//!
//! These tests are skipped (as no-op passing tests) unless
//! `GH_SECRETS_LIVE_TEST=1` and `GH_TOKEN` are both set. They share a single
//! sandbox repo (`gh-secrets-e2e-sandbox`) on the authenticated user's
//! account, isolate themselves via a per-test unique secret-name prefix, and
//! clean up the secrets they create in `Drop`.
//!
//! Run with: `just test-live` (or directly:
//! `GH_SECRETS_LIVE_TEST=1 GH_TOKEN=... cargo test --test e2e_live`).

mod live_common;

use std::process::Command as StdCommand;

use live_common::{live_enabled, token, LiveSession, LIVE_ENV};
use predicates::str::contains;
use tempfile::TempDir;

macro_rules! skip_if_no_live {
    () => {
        if !live_enabled() {
            eprintln!("skip: live test (set {LIVE_ENV}=1 + GH_TOKEN to run)");
            return;
        }
    };
}

/// Configure a profile, push a secret, and verify the GitHub API reports it.
#[test]
fn live_sync_creates_secret_visible_via_api() {
    skip_if_no_live!();
    let s = LiveSession::new("create");
    let tok = token();
    let name = s.secret_name("KEY");

    s.cmd().args(["token", tok.as_str()]).assert().success();
    s.cmd().args(["repo", "add", &s.repo]).assert().success();
    s.cmd()
        .args(["secrets", "add", &name, "live-test-value-1"])
        .assert()
        .success();
    s.cmd()
        .args(["secrets", "sync"])
        .assert()
        .success()
        .stdout(contains(format!("created '{name}' in {}", s.repo)));

    let remote = s.remote_secret_names();
    assert!(
        remote.contains(&name),
        "expected {name} in remote secret list, got {remote:?}"
    );
}

/// After an initial sync, an immediate resync against the real API must do
/// nothing. This is the central UX promise of the tool.
#[test]
fn live_noop_resync_returns_nothing_to_do() {
    skip_if_no_live!();
    let s = LiveSession::new("noop");
    let tok = token();
    let name = s.secret_name("KEY");

    s.cmd().args(["token", tok.as_str()]).assert().success();
    s.cmd().args(["repo", "add", &s.repo]).assert().success();
    s.cmd()
        .args(["secrets", "add", &name, "live-test-value-2"])
        .assert()
        .success();
    s.cmd().args(["secrets", "sync"]).assert().success();
    s.cmd()
        .args(["secrets", "sync"])
        .assert()
        .success()
        .stdout(contains("nothing to do"));
}

/// Updating a value locally must propagate on the next sync. GitHub's
/// `updated_at` for the secret advances when we PUT a new ciphertext, so we
/// use that as our remote witness.
#[test]
fn live_update_value_propagates_on_resync() {
    skip_if_no_live!();
    let s = LiveSession::new("update");
    let tok = token();
    let name = s.secret_name("KEY");

    s.cmd().args(["token", tok.as_str()]).assert().success();
    s.cmd().args(["repo", "add", &s.repo]).assert().success();
    s.cmd()
        .args(["secrets", "add", &name, "v1"])
        .assert()
        .success();
    s.cmd().args(["secrets", "sync"]).assert().success();
    let first = s
        .remote_secret(&name)
        .expect("secret exists after first sync");
    let first_updated = first["updated_at"]
        .as_str()
        .expect("updated_at present")
        .to_string();

    // Pause long enough that GitHub's second-resolution `updated_at` is
    // guaranteed to advance even if our second PUT lands very fast.
    std::thread::sleep(std::time::Duration::from_secs(2));

    s.cmd()
        .args(["secrets", "add", &name, "v2"])
        .assert()
        .success();
    s.cmd()
        .args(["secrets", "sync"])
        .assert()
        .success()
        .stdout(contains(format!("updated '{name}' in {}", s.repo)));
    let second = s
        .remote_secret(&name)
        .expect("secret still exists after update");
    let second_updated = second["updated_at"]
        .as_str()
        .expect("updated_at present")
        .to_string();
    assert!(
        second_updated > first_updated,
        "expected updated_at to advance: first={first_updated} second={second_updated}"
    );
}

/// Bad token first → real 401 from GitHub → rotate to a good token → success.
/// Proves the error message guides the user to the fix and that the fix works.
#[test]
fn live_recovers_from_invalid_token() {
    skip_if_no_live!();
    let s = LiveSession::new("rotate");
    let tok = token();
    let name = s.secret_name("KEY");

    s.cmd()
        .args(["token", "ghp_obviously_invalid_token_for_e2e_testing"])
        .assert()
        .success();
    s.cmd().args(["repo", "add", &s.repo]).assert().success();
    s.cmd()
        .args(["secrets", "add", &name, "v1"])
        .assert()
        .success();
    s.cmd()
        .args(["secrets", "sync"])
        .assert()
        .failure()
        .stderr(contains("401"));

    s.cmd().args(["token", tok.as_str()]).assert().success();
    s.cmd()
        .args(["secrets", "sync"])
        .assert()
        .success()
        .stdout(contains(format!("created '{name}' in {}", s.repo)));
    assert!(s.remote_secret_names().contains(&name));
}

/// Per-repo override path: when both a global and a repo-scoped secret exist
/// with the same name, sync uses the override. We can't read the value back,
/// but we exercise the code path end-to-end and confirm the resulting secret
/// is present on the remote.
#[test]
fn live_repo_override_wins_during_sync() {
    skip_if_no_live!();
    let s = LiveSession::new("override");
    let tok = token();
    let name = s.secret_name("KEY");

    s.cmd().args(["token", tok.as_str()]).assert().success();
    s.cmd().args(["repo", "add", &s.repo]).assert().success();
    s.cmd()
        .args(["secrets", "add", &name, "global-value"])
        .assert()
        .success();
    s.cmd()
        .args(["secrets", "add", &name, "override-value", &s.repo])
        .assert()
        .success();
    s.cmd()
        .args(["secrets", "sync"])
        .assert()
        .success()
        .stdout(contains(format!("created '{name}' in {}", s.repo)));
    assert!(s.remote_secret_names().contains(&name));
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
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/install.sh");

    let output = StdCommand::new("sh")
        .arg(script)
        .arg("--to")
        .arg(bindir.path())
        // The script reads GITHUB_TOKEN (not GH_TOKEN) to authenticate the
        // "latest release" API call and lift the unauthenticated rate limit.
        .env("GITHUB_TOKEN", token())
        .output()
        .expect("run scripts/install.sh");

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

/// Local-only removal must NOT touch the remote secret. The CLI deliberately
/// stays out of GitHub-side cleanup: if a user wants the remote secret gone
/// too, they delete it themselves.
#[test]
fn live_local_remove_does_not_delete_remote() {
    skip_if_no_live!();
    let s = LiveSession::new("local_remove");
    let tok = token();
    let name = s.secret_name("KEY");

    s.cmd().args(["token", tok.as_str()]).assert().success();
    s.cmd().args(["repo", "add", &s.repo]).assert().success();
    s.cmd()
        .args(["secrets", "add", &name, "v1"])
        .assert()
        .success();
    s.cmd().args(["secrets", "sync"]).assert().success();
    assert!(s.remote_secret_names().contains(&name));

    s.cmd()
        .args(["secrets", "remove", &name])
        .assert()
        .success();
    // After the local remove, the next sync has nothing to push.
    s.cmd()
        .args(["secrets", "sync"])
        .assert()
        .success()
        .stdout(contains("nothing to do"));
    assert!(
        s.remote_secret_names().contains(&name),
        "removing locally must not delete the remote secret"
    );
}
