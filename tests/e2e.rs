//! End-to-end tests: drive the compiled `gh-secrets` binary the way a user
//! does, against a mocked GitHub API. The compiled binary is the same artifact
//! the user runs, so this is the highest-fidelity local check we have short of
//! a real GitHub round-trip (`tests/e2e_live.rs`).
//!
//! Covers: local CRUD round-trips, happy-path sync + no-op resync, token
//! rotation recovery, profile isolation, `record fill`/`reset` semantics,
//! `repo bootstrap` from a discovered repo list (with and without an exclude
//! list), per-repo override during sync, `check` reporting drift, public-key
//! 404 with an actionable error, profile-delete on-disk cleanup, and a
//! ciphertext-shape assertion on the PUT body so we catch regressions in the
//! seal step without decrypting anything.

mod common;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use common::{fake_pubkey_b64, E2eHarness};
use predicates::str::contains;
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path_regex};
use wiremock::{Mock, ResponseTemplate};

/// Local-only flows: token, repo include/exclude, secrets add/remove. No
/// network involved. Catches regressions in the config persistence layer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_local_state_persists_across_invocations() {
    let h = E2eHarness::new().await;

    h.cmd().args(["token", "ghp_test"]).assert().success();

    h.cmd()
        .args(["repo", "add", "owner/repo1"])
        .assert()
        .success();
    h.cmd()
        .args(["repo", "add", "owner/repo2"])
        .assert()
        .success();
    // Duplicate add must fail; the error has to point at the conflict.
    h.cmd()
        .args(["repo", "add", "owner/repo1"])
        .assert()
        .failure()
        .stderr(contains("already included"));

    // Cannot move a repo to excluded while it's still included.
    h.cmd()
        .args(["repo", "add-exclude", "owner/repo1"])
        .assert()
        .failure()
        .stderr(contains("included list"));

    h.cmd()
        .args(["secrets", "add", "API_KEY", "value-1"])
        .assert()
        .success()
        .stdout(contains("created"));
    // Re-add updates.
    h.cmd()
        .args(["secrets", "add", "API_KEY", "value-2"])
        .assert()
        .success()
        .stdout(contains("updated"));
    // Repo-scoped override.
    h.cmd()
        .args(["secrets", "add", "API_KEY", "override", "owner/repo1"])
        .assert()
        .success();

    h.cmd()
        .args(["secrets", "remove", "API_KEY"])
        .assert()
        .success();
    // Removing a missing global secret is an error.
    h.cmd()
        .args(["secrets", "remove", "API_KEY"])
        .assert()
        .failure()
        .stderr(contains("was not defined"));
}

/// Happy path: push a global secret to two repos, then re-sync and confirm it
/// is a no-op.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_sync_happy_path_then_noop_resync() {
    let h = E2eHarness::new().await;

    Mock::given(method("GET"))
        .and(path_regex(
            r"^/repos/[^/]+/[^/]+/actions/secrets/public-key$",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"key_id": "kid-1", "key": fake_pubkey_b64()})),
        )
        .mount(&h.server)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(r"^/repos/[^/]+/[^/]+/actions/secrets/[^/]+$"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&h.server)
        .await;

    h.cmd().args(["token", "good"]).assert().success();
    h.cmd()
        .args(["repo", "add", "owner/repo1"])
        .assert()
        .success();
    h.cmd()
        .args(["repo", "add", "owner/repo2"])
        .assert()
        .success();
    h.cmd()
        .args(["secrets", "add", "API_KEY", "v1"])
        .assert()
        .success();

    // Initial sync: both repos get the secret.
    h.cmd()
        .args(["secrets", "sync"])
        .assert()
        .success()
        .stdout(contains("created 'API_KEY' in owner/repo1"))
        .stdout(contains("created 'API_KEY' in owner/repo2"));

    // Re-sync: nothing changed, so nothing happens.
    h.cmd()
        .args(["secrets", "sync"])
        .assert()
        .success()
        .stdout(contains("nothing to do"));
}

/// Failure/recovery: GitHub rejects a bad token; the user rotates it and the
/// next sync succeeds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_sync_recovers_after_token_rotation() {
    let h = E2eHarness::new().await;

    // Bad token: 401 on the public-key fetch.
    Mock::given(method("GET"))
        .and(path_regex(
            r"^/repos/[^/]+/[^/]+/actions/secrets/public-key$",
        ))
        .and(header("authorization", "Bearer bad"))
        .respond_with(
            ResponseTemplate::new(401).set_body_string(r#"{"message":"Bad credentials"}"#),
        )
        .mount(&h.server)
        .await;

    // Good token: 200 + a real-shaped public key payload.
    Mock::given(method("GET"))
        .and(path_regex(
            r"^/repos/[^/]+/[^/]+/actions/secrets/public-key$",
        ))
        .and(header("authorization", "Bearer good"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"key_id": "kid-1", "key": fake_pubkey_b64()})),
        )
        .mount(&h.server)
        .await;

    Mock::given(method("PUT"))
        .and(path_regex(r"^/repos/[^/]+/[^/]+/actions/secrets/[^/]+$"))
        .and(header("authorization", "Bearer good"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&h.server)
        .await;

    h.cmd().args(["token", "bad"]).assert().success();
    h.cmd()
        .args(["repo", "add", "owner/repo1"])
        .assert()
        .success();
    h.cmd()
        .args(["secrets", "add", "API_KEY", "v1"])
        .assert()
        .success();

    // First sync fails with a clear message naming the 401.
    h.cmd()
        .args(["secrets", "sync"])
        .assert()
        .failure()
        .stderr(contains("401"))
        .stderr(contains("gh-secrets token"));

    // Rotate to the good token; retry succeeds.
    h.cmd().args(["token", "good"]).assert().success();
    h.cmd()
        .args(["secrets", "sync"])
        .assert()
        .success()
        .stdout(contains("created 'API_KEY' in owner/repo1"));
}

/// Profile lifecycle: create, switch, isolate state, delete.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_profiles_isolate_state() {
    let h = E2eHarness::new().await;

    h.cmd().args(["token", "default-token"]).assert().success();
    h.cmd()
        .args(["secrets", "add", "A", "1"])
        .assert()
        .success();

    h.cmd()
        .args(["profile", "create", "work"])
        .assert()
        .success();
    h.cmd().args(["profile", "set", "work"]).assert().success();
    // Reserved name is rejected.
    h.cmd()
        .args(["profile", "create", "app"])
        .assert()
        .failure()
        .stderr(contains("reserved"));

    // 'work' profile starts empty: removing A errors.
    h.cmd()
        .args(["secrets", "remove", "A"])
        .assert()
        .failure()
        .stderr(contains("was not defined"));

    // Cannot delete the active profile.
    h.cmd()
        .args(["profile", "delete", "work"])
        .assert()
        .failure()
        .stderr(contains("currently active"));

    // Switch back to default and confirm A is still there.
    h.cmd()
        .args(["profile", "set", "default"])
        .assert()
        .success();
    h.cmd().args(["secrets", "remove", "A"]).assert().success();
}

/// `record fill` pre-marks every (repo, secret) as already synced. The next
/// sync should be a no-op because the local timestamps haven't moved.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_record_fill_makes_next_sync_a_noop() {
    let h = E2eHarness::new().await;

    // No mocks for the secret endpoints: a no-op sync should never hit them,
    // so missing mocks here are an assertion that we never reached out.

    h.cmd().args(["token", "good"]).assert().success();
    h.cmd()
        .args(["repo", "add", "owner/repo1"])
        .assert()
        .success();
    h.cmd()
        .args(["secrets", "add", "API_KEY", "v1"])
        .assert()
        .success();
    h.cmd().args(["record", "fill"]).assert().success();

    h.cmd()
        .args(["secrets", "sync"])
        .assert()
        .success()
        .stdout(contains("nothing to do"));

    // Nothing should have hit GitHub.
    let reqs = h.server.received_requests().await.unwrap_or_default();
    assert!(
        reqs.is_empty(),
        "expected no GitHub calls after record fill + sync, got {} requests",
        reqs.len()
    );
}

/// `record reset` wipes the sync log. Even an unchanged secret should be
/// re-pushed on the next sync.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_record_reset_forces_resync() {
    let h = E2eHarness::new().await;

    Mock::given(method("GET"))
        .and(path_regex(
            r"^/repos/[^/]+/[^/]+/actions/secrets/public-key$",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"key_id": "kid-1", "key": fake_pubkey_b64()})),
        )
        .mount(&h.server)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(r"^/repos/[^/]+/[^/]+/actions/secrets/[^/]+$"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&h.server)
        .await;

    h.cmd().args(["token", "good"]).assert().success();
    h.cmd()
        .args(["repo", "add", "owner/repo1"])
        .assert()
        .success();
    h.cmd()
        .args(["secrets", "add", "API_KEY", "v1"])
        .assert()
        .success();

    h.cmd().args(["secrets", "sync"]).assert().success();
    // Resync without reset: no-op.
    h.cmd()
        .args(["secrets", "sync"])
        .assert()
        .success()
        .stdout(contains("nothing to do"));
    // After reset, the same secret should ship again.
    h.cmd().args(["record", "reset"]).assert().success();
    h.cmd()
        .args(["secrets", "sync"])
        .assert()
        .success()
        .stdout(contains("created 'API_KEY' in owner/repo1"));
}

/// `repo bootstrap` discovers repos via /user/repos and adds them to the
/// included list. Excluded repos must be skipped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_repo_bootstrap_includes_discovered_minus_excluded() {
    let h = E2eHarness::new().await;

    Mock::given(method("GET"))
        .and(path_regex(r"^/user/repos$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"full_name": "owner/alpha"},
            {"full_name": "owner/beta"},
            {"full_name": "owner/gamma"},
        ])))
        .mount(&h.server)
        .await;

    h.cmd().args(["token", "good"]).assert().success();
    h.cmd()
        .args(["repo", "add-exclude", "owner/beta"])
        .assert()
        .success();
    h.cmd()
        .args(["repo", "bootstrap"])
        .assert()
        .success()
        .stdout(contains("owner/alpha"))
        .stdout(contains("owner/gamma"))
        .stdout(contains("2 repo(s) added"));
}

/// When both a global and a repo-scoped secret exist with the same name, sync
/// must use the per-repo override. The mock counts PUTs; we expect one per
/// included repo, and we verify the encrypted_value on the wire looks like a
/// libsodium sealed box (base64 of ephemeral pubkey + MAC + ciphertext).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_sync_uses_per_repo_override_and_seals_value() {
    let h = E2eHarness::new().await;

    Mock::given(method("GET"))
        .and(path_regex(
            r"^/repos/[^/]+/[^/]+/actions/secrets/public-key$",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"key_id": "kid-1", "key": fake_pubkey_b64()})),
        )
        .mount(&h.server)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(r"^/repos/[^/]+/[^/]+/actions/secrets/[^/]+$"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&h.server)
        .await;

    h.cmd().args(["token", "good"]).assert().success();
    h.cmd()
        .args(["repo", "add", "owner/repo1"])
        .assert()
        .success();
    h.cmd()
        .args(["repo", "add", "owner/repo2"])
        .assert()
        .success();
    h.cmd()
        .args(["secrets", "add", "API_KEY", "global-value-with-some-length"])
        .assert()
        .success();
    h.cmd()
        .args([
            "secrets",
            "add",
            "API_KEY",
            "override-much-longer-than-the-global-value-on-purpose",
            "owner/repo1",
        ])
        .assert()
        .success();
    h.cmd().args(["secrets", "sync"]).assert().success();

    let puts: Vec<_> = h
        .server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.method.as_str().eq_ignore_ascii_case("PUT"))
        .collect();
    assert_eq!(puts.len(), 2, "expected one PUT per included repo");

    for p in &puts {
        let body: Value = serde_json::from_slice(&p.body).expect("PUT body is json");
        let key_id = body["key_id"].as_str().expect("key_id present");
        assert_eq!(key_id, "kid-1");
        let enc = body["encrypted_value"]
            .as_str()
            .expect("encrypted_value present");
        let raw = B64.decode(enc).expect("encrypted_value is base64");
        // Plaintext was never empty, so the floor on a sealed-box is the
        // ephemeral pubkey (32) + Poly1305 tag (16) + at least one plaintext
        // byte. Assert the structural floor rather than an exact length so we
        // don't pin to a specific message size.
        assert!(
            raw.len() > 32 + 16,
            "ciphertext too short: {} bytes",
            raw.len()
        );
        assert!(!enc.contains("global-value"), "plaintext leaked into body");
        assert!(!enc.contains("override-much"), "plaintext leaked into body");
    }
}

/// A 404 on the public-key fetch must surface as a clear error pointing at the
/// repo name, not a low-level reqwest message.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_sync_surfaces_public_key_404_clearly() {
    let h = E2eHarness::new().await;

    Mock::given(method("GET"))
        .and(path_regex(
            r"^/repos/[^/]+/[^/]+/actions/secrets/public-key$",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_string(r#"{"message":"Not Found"}"#))
        .mount(&h.server)
        .await;

    h.cmd().args(["token", "good"]).assert().success();
    h.cmd()
        .args(["repo", "add", "owner/missing"])
        .assert()
        .success();
    h.cmd()
        .args(["secrets", "add", "API_KEY", "v1"])
        .assert()
        .success();

    h.cmd()
        .args(["secrets", "sync"])
        .assert()
        .failure()
        .stderr(contains("404"))
        .stderr(contains("owner/missing"));
}

/// `check` should call /user/repos, report discovered-but-unincluded repos,
/// and list stale secrets (those without a sync record).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_check_reports_new_repos_and_unsynced_secrets() {
    let h = E2eHarness::new().await;

    Mock::given(method("GET"))
        .and(path_regex(r"^/user/repos$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"full_name": "owner/known"},
            {"full_name": "owner/brand-new"},
        ])))
        .mount(&h.server)
        .await;

    h.cmd().args(["token", "good"]).assert().success();
    h.cmd()
        .args(["repo", "add", "owner/known"])
        .assert()
        .success();
    h.cmd()
        .args(["secrets", "add", "API_KEY", "v1"])
        .assert()
        .success();

    h.cmd()
        .args(["check"])
        .assert()
        .success()
        .stdout(contains("owner/brand-new"))
        .stdout(contains("API_KEY"));
}

/// Deleting a profile must drop the on-disk profile file too, not just the
/// entry in app.json. Otherwise a future `profile create <same-name>` would
/// silently inherit the old state.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_profile_delete_removes_file_on_disk() {
    let h = E2eHarness::new().await;

    h.cmd()
        .args(["profile", "create", "scratch"])
        .assert()
        .success();
    let scratch_file = h.home.path().join("profiles").join("scratch.json");
    assert!(
        scratch_file.exists(),
        "profile file should exist after create"
    );

    // Cannot delete the active profile; we are still on `default`, so this
    // should succeed.
    h.cmd()
        .args(["profile", "delete", "scratch"])
        .assert()
        .success();
    assert!(
        !scratch_file.exists(),
        "profile file should be gone after delete"
    );
}
