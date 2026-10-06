//! Live e2e tests against a **real, isolated** Bitwarden account.
//!
//! These prove the half of the pipeline the wiremock GitHub suite can't: that
//! `gh-secrets` actually authenticates to Bitwarden (api-key login + master
//! password unlock), syncs the vault, and pulls real values out of real items
//! — password, username, notes, and custom fields — then pushes them to a
//! destination, all without ever printing a value.
//!
//! They are skipped (as no-op passing tests) unless `GH_SECRETS_LIVE_TEST=1`
//! and the isolated account's api-key credentials are present:
//! `GH_SECRETS_BW_E2E_CLIENT_ID`, `GH_SECRETS_BW_E2E_CLIENT_SECRET`,
//! `GH_SECRETS_BW_E2E_PASSWORD`. The account exists solely for this suite, so
//! seeding and deleting items in it is safe; each test isolates itself with a
//! unique item-name prefix and a throwaway `bw` app-data dir, and cleans up the
//! items it creates in `Drop`. See `tests/live_bw_common/mod.rs` for the why.
//!
//! Run with: `just test-live-bitwarden` (which expects the three credentials in
//! the environment; `scripts/bw-e2e-env.sh` populates them from the developer's
//! own vault where the isolated account's keys are stored).

#[macro_use]
mod live_bw_common;

use live_bw_common::BwLiveSession;
use predicates::str::contains;
use tempfile::TempDir;

/// Read this session's env-file destination back as a string.
fn dest_body(s: &BwLiveSession) -> String {
    std::fs::read_to_string(s.dir.path().join("out.env")).expect("read out.env destination")
}

/// A login item's password reaches an env-file destination, and an immediate
/// resync of the unchanged value is a genuine no-op — the central UX promise,
/// proven end-to-end against real Bitwarden.
#[test]
fn live_bw_sync_password_then_noop_resync() {
    skip_if_no_bw_live!();
    let s = BwLiveSession::new("syncpw");
    let pw = "live-bw-password-value-1";
    let item = s.seed_login(
        "LOGIN",
        "unused-username",
        pw,
        "unused-notes",
        ("CUSTOM", "unused"),
    );

    let assert = s
        .cmd()
        .args([
            "sync",
            "--from",
            "bitwarden",
            "--to",
            "env:out.env",
            "--secret",
            &format!("GHSE2E_PW={item}"),
        ])
        .assert()
        .success();
    // The value must never appear in the CLI's own output.
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(!stdout.contains(pw), "the secret value leaked to stdout");

    assert!(
        dest_body(&s).contains(&format!("GHSE2E_PW=\"{pw}\"")),
        "expected the fetched password in the env destination"
    );

    // Re-sync: same source value, recorded state → nothing to do.
    s.cmd()
        .args([
            "sync",
            "--from",
            "bitwarden",
            "--to",
            "env:out.env",
            "--secret",
            &format!("GHSE2E_PW={item}"),
        ])
        .assert()
        .success()
        .stdout(contains("nothing to do"));
}

/// Every supported field selector pulls the right value off one real item:
/// the default `password`, plus `#username`, `#notes`, and `#fields.<NAME>`.
#[test]
fn live_bw_field_selectors_extract_each_field() {
    skip_if_no_bw_live!();
    let s = BwLiveSession::new("fields");
    let (user, pw, notes, custom) = (
        "username-value-2",
        "password-value-2",
        "notes-value-2",
        "custom-field-value-2",
    );
    let item = s.seed_login("ITEM", user, pw, notes, ("API_URL", custom));

    s.cmd()
        .args([
            "sync",
            "--from",
            "bitwarden",
            "--to",
            "env:out.env",
            "--secret",
            &format!("PW={item}"),
            "--secret",
            &format!("USER={item}#username"),
            "--secret",
            &format!("NOTE={item}#notes"),
            "--secret",
            &format!("CUST={item}#fields.API_URL"),
        ])
        .assert()
        .success();

    let body = dest_body(&s);
    for (key, value) in [
        ("PW", pw),
        ("USER", user),
        ("NOTE", notes),
        ("CUST", custom),
    ] {
        assert!(
            body.contains(&format!("{key}=\"{value}\"")),
            "expected {key} from its field selector; got:\n{body}"
        );
    }
}

/// `--default-field` changes what an unselected secret extracts, while an
/// explicit `#field` selector still overrides it per secret.
#[test]
fn live_bw_default_field_flag_changes_the_default() {
    skip_if_no_bw_live!();
    let s = BwLiveSession::new("deffield");
    let (user, pw, notes) = ("username-value-5", "password-value-5", "notes-value-5");
    let item = s.seed_login("DEFF", user, pw, notes, ("C", "v"));

    s.cmd()
        .args([
            "sync",
            "--from",
            "bitwarden",
            "--default-field",
            "notes",
            "--to",
            "env:out.env",
            "--secret",
            &format!("DEFAULTED={item}"),
            "--secret",
            &format!("OVERRIDDEN={item}#username"),
        ])
        .assert()
        .success();

    let body = dest_body(&s);
    assert!(
        body.contains(&format!("DEFAULTED=\"{notes}\"")),
        "expected the default-field value (notes); got:\n{body}"
    );
    assert!(
        body.contains(&format!("OVERRIDDEN=\"{user}\"")),
        "expected the per-secret selector to override the default; got:\n{body}"
    );
}

/// `source list` enumerates the vault by contacting Bitwarden directly, and
/// prints item names but never a value.
#[test]
fn live_bw_source_list_shows_item_names_not_values() {
    skip_if_no_bw_live!();
    let s = BwLiveSession::new("list");
    let secret_pw = "must-not-be-printed-3";
    let item = s.seed_login("LISTME", "u", secret_pw, "n", ("C", "v"));

    let assert = s
        .cmd()
        .args(["source", "list", "--from", "bitwarden"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();

    assert!(
        stdout.contains(&item),
        "expected the seeded item name in `source list`; got:\n{stdout}"
    );
    assert!(
        !stdout.contains(secret_pw),
        "`source list` must never print a value"
    );
}

/// The full cold path through `gh-secrets` itself: pointed at a *fresh* `bw`
/// app-data dir (no prior login), it must perform `bw login --apikey` → unlock
/// → sync → fetch on its own and still land the value at the destination.
#[test]
fn live_bw_cold_login_full_path_fetches_value() {
    skip_if_no_bw_live!();
    let s = BwLiveSession::new("cold");
    let pw = "cold-login-password-4";
    let item = s.seed_login("COLD", "u", pw, "n", ("C", "v"));

    // A pristine app-data dir: gh-secrets sees `unauthenticated` and runs the
    // whole login/unlock/sync chain itself before fetching.
    let fresh = TempDir::new().expect("fresh appdata dir");
    s.cmd_in_appdata(fresh.path())
        .args([
            "sync",
            "--from",
            "bitwarden",
            "--to",
            "env:out.env",
            "--secret",
            &format!("COLDPW={item}"),
        ])
        .assert()
        .success();

    assert!(
        dest_body(&s).contains(&format!("COLDPW=\"{pw}\"")),
        "expected the value fetched via a cold gh-secrets login"
    );
}

/// A wrong master password surfaces a precise unlock error (never a hang), with
/// no value written. Exercises the real Bitwarden unlock-failure path.
#[test]
fn live_bw_wrong_master_password_errors_clearly() {
    skip_if_no_bw_live!();
    let s = BwLiveSession::new("wrongpw");
    let item = s.seed_login("WPW", "u", "p", "n", ("C", "v"));

    s.cmd()
        .env("BW_PASSWORD", "this-is-not-the-master-password")
        .args([
            "sync",
            "--from",
            "bitwarden",
            "--to",
            "env:out.env",
            "--secret",
            &format!("WPW={item}"),
        ])
        .assert()
        .failure()
        .stderr(contains("unlock"));

    assert!(
        !s.dir.path().join("out.env").exists(),
        "nothing should be written when the unlock fails"
    );
}
