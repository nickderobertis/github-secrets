//! The sandbox journeys: each drives the unified `sync` command with an
//! env-file source and a `github:` destination at the configured sandbox repo —
//! the same pipeline a user runs, with the GitHub token resolving from
//! `GH_TOKEN` exactly as documented. `tests/e2e_live.rs` runs them against the
//! real GitHub API; `tests/sandbox_fixture.rs` runs the same functions against
//! a loopback double to prove the sandbox identity comes from configuration.

use predicates::str::contains;

use super::{token, LiveSession};

/// Sync a secret from an env-file source to the sandbox repo and verify the
/// GitHub API reports it.
pub fn sync_creates_secret_visible_via_api() {
    let s = LiveSession::new("create");
    let name = s.secret_name("KEY");
    let from = s.write_source_env(&[(&name, "live-test-value-1")]);

    s.cmd()
        .args([
            "sync",
            "--from",
            &from,
            "--to",
            &format!("github:{}", s.repo),
            "--secret",
            &name,
        ])
        .assert()
        .success()
        .stdout(contains(format!("github:{}: created '{name}'", s.repo)));

    let remote = s.remote_secret_names();
    assert!(
        remote.contains(&name),
        "expected {name} in remote secret list, got {remote:?}"
    );
}

/// After an initial sync, an immediate resync against the real API must do
/// nothing. This is the central UX promise of the tool.
pub fn noop_resync_returns_nothing_to_do() {
    let s = LiveSession::new("noop");
    let name = s.secret_name("KEY");
    let from = s.write_source_env(&[(&name, "live-test-value-2")]);
    let to = format!("github:{}", s.repo);
    let args = ["sync", "--from", &from, "--to", &to, "--secret", &name];

    s.cmd().args(args).assert().success();
    s.cmd()
        .args(args)
        .assert()
        .success()
        .stdout(contains("nothing to do"));
}

/// Updating the source value must propagate on the next sync. GitHub's
/// `updated_at` for the secret advances when we PUT a new ciphertext, so we
/// use that as our remote witness.
pub fn update_value_propagates_on_resync() {
    let s = LiveSession::new("update");
    let name = s.secret_name("KEY");
    let from = s.write_source_env(&[(&name, "v1")]);
    let to = format!("github:{}", s.repo);
    let args = ["sync", "--from", &from, "--to", &to, "--secret", &name];

    s.cmd().args(args).assert().success();
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

    s.write_source_env(&[(&name, "v2")]);
    s.cmd()
        .args(args)
        .assert()
        .success()
        .stdout(contains(format!("github:{}: updated '{name}'", s.repo)));
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

/// Bad token first → real 401 from GitHub → rerun with the good token →
/// success. Proves the error message guides the user to the fix and that the
/// fix works.
pub fn recovers_from_invalid_token() {
    let s = LiveSession::new("rotate");
    let name = s.secret_name("KEY");
    let from = s.write_source_env(&[(&name, "v1")]);
    let to = format!("github:{}", s.repo);
    let args = ["sync", "--from", &from, "--to", &to, "--secret", &name];

    s.cmd()
        .env("GH_TOKEN", "ghp_obviously_invalid_token_for_e2e_testing")
        .args(args)
        .assert()
        .failure()
        .stderr(contains("401"));

    s.cmd()
        .env("GH_TOKEN", token())
        .args(args)
        .assert()
        .success()
        .stdout(contains(format!("github:{}: created '{name}'", s.repo)));
    assert!(s.remote_secret_names().contains(&name));
}

/// Removing a secret from the pipeline must NOT touch the remote secret. The
/// CLI deliberately stays out of GitHub-side cleanup: if a user wants the
/// remote secret gone too, they delete it themselves.
pub fn undeclared_secret_is_not_deleted_remotely() {
    let s = LiveSession::new("undeclare");
    let name = s.secret_name("KEY");
    let from = s.write_source_env(&[(&name, "v1")]);
    let to = format!("github:{}", s.repo);

    s.cmd()
        .args(["sync", "--from", &from, "--to", &to, "--secret", &name])
        .assert()
        .success();
    assert!(s.remote_secret_names().contains(&name));

    // Same pipeline without the secret declared: nothing to push, and the
    // remote secret survives.
    s.cmd()
        .args(["sync", "--from", &from, "--to", &to])
        .assert()
        .success()
        .stdout(contains("nothing to do"));
    assert!(
        s.remote_secret_names().contains(&name),
        "an undeclared secret must not be deleted remotely"
    );
}
