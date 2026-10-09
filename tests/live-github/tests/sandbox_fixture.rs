//! Proves the live suite's sandbox repo is configuration, offline: each test
//! points `GH_SECRETS_LIVE_API_BASE` at a loopback GitHub double that serves a
//! synthetic repo, sets `GH_SECRETS_E2E_SANDBOX_REPO` to that identity, and runs
//! the very journey functions `tests/e2e_live.rs` runs against GitHub — so the
//! sandbox requests are observed landing on the configured repo. With the key
//! unset or wrong, the suite fails naming the key instead of skipping.
//!
//! Each test sets process env, which is safe because nextest runs one test per
//! process (the repo's required runner).

mod fake_github;
mod live_common;

use std::panic;

use fake_github::{FakeGithub, KEY_ID};
use live_common::{journeys, LiveSession, API_BASE_ENV, LIVE_ENV, SANDBOX_REPO_ENV, TOKEN_ENV};

/// A synthetic identity: never a real repository.
const SYNTHETIC_REPO: &str = "hiddenco/quietharbor";
const FAKE_TOKEN: &str = "fixture-token";

/// Configure the live suite exactly as `just test-live` would, but aimed at a
/// double. `repo` is what the key is set to (`None` leaves it unset).
fn configure(fake: &FakeGithub, repo: Option<&str>) {
    std::env::set_var(LIVE_ENV, "1");
    std::env::set_var(TOKEN_ENV, FAKE_TOKEN);
    std::env::set_var(API_BASE_ENV, fake.uri());
    match repo {
        Some(r) => std::env::set_var(SANDBOX_REPO_ENV, r),
        None => std::env::remove_var(SANDBOX_REPO_ENV),
    }
}

/// Run one journey against the double under the synthetic identity, then
/// check where its requests went: every one at the configured repo, including
/// the binary's own secret PUTs.
fn run_against_configured_repo(journey: fn()) -> Vec<String> {
    let fake = FakeGithub::start(SYNTHETIC_REPO, FAKE_TOKEN);
    configure(&fake, Some(SYNTHETIC_REPO));
    journey();
    let requests = fake.requests();
    let prefix = format!("/repos/{SYNTHETIC_REPO}");
    assert!(
        requests
            .iter()
            .all(|r| r.split_once(' ').unwrap().1.starts_with(&prefix)),
        "a request missed the configured repo: {requests:#?}"
    );
    let put = format!("PUT {prefix}/actions/secrets/E2E_");
    assert!(
        requests.iter().any(|r| r.starts_with(&put)),
        "the binary never pushed to the configured repo: {requests:#?}"
    );
    requests
}

/// The panic message of `f`, which must panic.
fn panic_message(f: impl FnOnce() + panic::UnwindSafe) -> String {
    let err = panic::catch_unwind(f).expect_err("expected a failure, not a pass or skip");
    err.downcast_ref::<String>()
        .cloned()
        .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default()
}

#[test]
fn sync_creates_secret_reaches_the_configured_repo() {
    run_against_configured_repo(journeys::sync_creates_secret_visible_via_api);
}

#[test]
fn noop_resync_reaches_the_configured_repo() {
    run_against_configured_repo(journeys::noop_resync_returns_nothing_to_do);
}

#[test]
fn update_propagates_to_the_configured_repo() {
    run_against_configured_repo(journeys::update_value_propagates_on_resync);
}

#[test]
fn invalid_token_recovery_reaches_the_configured_repo() {
    let requests = run_against_configured_repo(journeys::recovers_from_invalid_token);
    // The bad-token attempt also hit the configured repo (and got the 401).
    let key = format!("GET /repos/{SYNTHETIC_REPO}/actions/secrets/public-key");
    assert!(requests.iter().filter(|r| **r == key).count() >= 2);
}

#[test]
fn undeclared_secret_survives_at_the_configured_repo() {
    run_against_configured_repo(journeys::undeclared_secret_is_not_deleted_remotely);
}

#[test]
fn unset_sandbox_key_fails_naming_it() {
    let fake = FakeGithub::start(SYNTHETIC_REPO, FAKE_TOKEN);
    configure(&fake, None);
    let msg = panic_message(journeys::sync_creates_secret_visible_via_api);
    assert!(
        msg.contains(SANDBOX_REPO_ENV),
        "message must name the key: {msg}"
    );
    assert!(
        fake.requests().is_empty(),
        "nothing may run without the key"
    );
}

#[test]
fn malformed_sandbox_key_fails_naming_it() {
    let fake = FakeGithub::start(SYNTHETIC_REPO, FAKE_TOKEN);
    // Not owner/name, or carrying URL metacharacters or dot segments that would
    // reshape the request paths the identity is interpolated into.
    for bad in [
        "quietharbor",
        "hiddenco/quietharbor/extra",
        "hiddenco/quietharbor?per_page=1",
        "hiddenco/quiet harbor",
        "hiddenco/..",
    ] {
        configure(&fake, Some(bad));
        let msg = panic_message(|| {
            LiveSession::new("malformed");
        });
        assert!(
            msg.contains(SANDBOX_REPO_ENV),
            "{bad:?}: message must name the key: {msg}"
        );
    }
    assert!(
        fake.requests().is_empty(),
        "a malformed key is refused before any call"
    );
}

#[test]
fn sandbox_key_naming_another_repo_fails_naming_it() {
    // The double serves only the synthetic repo; a key naming any other repo
    // must stop the suite at the reachability check, naming the key to fix.
    let fake = FakeGithub::start(SYNTHETIC_REPO, FAKE_TOKEN);
    configure(&fake, Some("hiddenco/otherharbor"));
    let msg = panic_message(journeys::sync_creates_secret_visible_via_api);
    assert!(
        msg.contains(SANDBOX_REPO_ENV),
        "message must name the key: {msg}"
    );
    assert_eq!(fake.requests(), vec!["GET /repos/hiddenco/otherharbor"]);
}

#[test]
fn the_double_rejects_a_malformed_secret_put() {
    // The double must catch a binary that sends GitHub a broken body, or the
    // journeys above could pass on requests GitHub itself would refuse.
    let fake = FakeGithub::start(SYNTHETIC_REPO, FAKE_TOKEN);
    let url = format!(
        "{}/repos/{SYNTHETIC_REPO}/actions/secrets/E2E_X",
        fake.uri()
    );
    let put = |body: serde_json::Value| {
        reqwest::blocking::Client::new()
            .put(&url)
            .bearer_auth(FAKE_TOKEN)
            .json(&body)
            .send()
            .expect("PUT to the double")
            .status()
            .as_u16()
    };
    assert_eq!(put(serde_json::json!({ "key_id": KEY_ID })), 422);
    assert_eq!(
        put(serde_json::json!({ "key_id": "other", "encrypted_value": "AAAA" })),
        422
    );
    assert_eq!(
        put(serde_json::json!({ "key_id": KEY_ID, "encrypted_value": "AAAA" })),
        422
    );
}

#[test]
fn a_public_sandbox_repo_fails_naming_the_key() {
    // Secrets pushed by the suite must land in a private repo; a key naming a
    // public one stops at the probe.
    let fake = FakeGithub::start_with(SYNTHETIC_REPO, FAKE_TOKEN, false);
    configure(&fake, Some(SYNTHETIC_REPO));
    let msg = panic_message(journeys::sync_creates_secret_visible_via_api);
    assert!(
        msg.contains(SANDBOX_REPO_ENV),
        "message must name the key: {msg}"
    );
    assert_eq!(
        fake.requests(),
        vec![format!("GET /repos/{SYNTHETIC_REPO}")]
    );
}

#[test]
fn a_non_loopback_api_override_is_refused_before_any_call() {
    // The override carries GH_TOKEN, so it may only name a local double.
    let fake = FakeGithub::start(SYNTHETIC_REPO, FAKE_TOKEN);
    configure(&fake, Some(SYNTHETIC_REPO));
    for bad in [
        "https://example.invalid",
        "http://127.0.0.1.example.invalid:80",
    ] {
        std::env::set_var(API_BASE_ENV, bad);
        let msg = panic_message(journeys::sync_creates_secret_visible_via_api);
        assert!(
            msg.contains(API_BASE_ENV),
            "{bad:?}: message must name the override: {msg}"
        );
    }
    assert!(fake.requests().is_empty());
}
