// llmlint: ignore[e2e_not_mocked] bw is the third-party Bitwarden CLI, not the code under test: this stand-in is spawned by the real gh-secrets binary through the real subprocess contract (argv, env, exit code, stdout), and the genuine bw call is the gh-secrets-live-bitwarden project's.
//! A stand-in for the Bitwarden CLI (`bw`), used by `tests/e2e_bitwarden.rs`.
//!
//! The offline suite copies this binary into a tempdir as `bw` (`bw.exe` on
//! Windows) and puts that directory first on `PATH`, so the real `gh-secrets`
//! binary spawns it exactly as it spawns the real CLI — same argv, same env
//! handoff, same exit-code/stdout/stderr contract. It answers the subcommands
//! `gh-secrets` uses (`status`, `login --apikey`, `unlock --raw --passwordenv`,
//! `sync`, `get item`, `list items`) from a JSON vault fixture named by
//! `FAKE_BW_STATE`, and appends one line per invocation (the argv) to
//! `FAKE_BW_LOG` so a test can assert which calls were made.
//!
//! Fixture shape (`client_id`, `client_secret`, `password`, `session` and
//! `items` required; the rest optional):
//! `{ "status": "unauthenticated"|"locked"|"unlocked", "client_id", "client_secret",
//!    "password", "session", "status_raw", "unlock_raw", "items": [<bw item>...] }`
//! `status_raw` / `unlock_raw` replace the normal stdout of those commands, to
//! drive `gh-secrets`' handling of malformed `bw` output.

use std::env;
use std::fs;
use std::io::Write;
use std::process::ExitCode;

use serde_json::{json, Value};

fn fail(msg: &str) -> ExitCode {
    eprintln!("{msg}");
    ExitCode::from(1)
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(state_path) = env::var_os("FAKE_BW_STATE") else {
        return fail("fake bw: FAKE_BW_STATE is not set");
    };
    if let Some(log) = env::var_os("FAKE_BW_LOG") {
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .expect("fake bw: open log");
        writeln!(f, "{}", args.join(" ")).expect("fake bw: write log");
    }
    let raw = fs::read_to_string(&state_path).expect("fake bw: read state");
    let mut state: Value = serde_json::from_str(&raw).expect("fake bw: parse state");
    // A fixture missing a credential must not let an unset env var "match" it.
    for key in ["client_id", "client_secret", "password", "session"] {
        if !state.get(key).is_some_and(Value::is_string) {
            return fail(&format!("fake bw: fixture must set \"{key}\" to a string"));
        }
    }
    if !state.get("items").is_some_and(Value::is_array) {
        return fail("fake bw: fixture must set \"items\" to an array");
    }
    let field = |s: &Value, k: &str| s.get(k).and_then(Value::as_str).map(String::from);
    let session_ok = || env::var("BW_SESSION").ok() == field(&state, "session");

    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["status"] => {
            match field(&state, "status_raw") {
                Some(raw) => println!("{raw}"),
                None => {
                    let status = field(&state, "status").unwrap_or_else(|| "locked".into());
                    println!("{}", json!({ "status": status }));
                }
            }
            ExitCode::SUCCESS
        }
        ["login", "--apikey"] => {
            if env::var("BW_CLIENTID").ok() != field(&state, "client_id")
                || env::var("BW_CLIENTSECRET").ok() != field(&state, "client_secret")
            {
                return fail("client_id or client_secret is incorrect. Try again.");
            }
            state["status"] = json!("locked");
            fs::write(&state_path, state.to_string()).expect("fake bw: write state");
            println!("You are logged in!");
            ExitCode::SUCCESS
        }
        ["unlock", "--raw", "--passwordenv", var] => {
            if env::var(var).ok() != field(&state, "password") {
                return fail("Invalid master password.");
            }
            match field(&state, "unlock_raw") {
                Some(raw) => print!("{raw}"),
                None => print!("{}", field(&state, "session").unwrap_or_default()),
            }
            ExitCode::SUCCESS
        }
        ["sync"] if session_ok() => {
            println!("Syncing complete.");
            ExitCode::SUCCESS
        }
        ["get", "item", id] if session_ok() => {
            let items = state["items"].as_array().cloned().unwrap_or_default();
            match items
                .iter()
                .find(|i| i["id"].as_str() == Some(id) || i["name"].as_str() == Some(id))
            {
                Some(item) => {
                    println!("{item}");
                    ExitCode::SUCCESS
                }
                None => fail("Not found."),
            }
        }
        ["list", "items", rest @ ..] if session_ok() => {
            let flag = |name: &str| {
                rest.iter()
                    .position(|a| *a == name)
                    .and_then(|i| rest.get(i + 1))
                    .map(|s| s.to_string())
            };
            let collection = flag("--collectionid");
            let organization = flag("--organizationid");
            let items: Vec<Value> = state["items"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|i| {
                    collection.as_ref().is_none_or(|c| {
                        i["collectionIds"]
                            .as_array()
                            .is_some_and(|ids| ids.iter().any(|v| v.as_str() == Some(c)))
                    }) && organization
                        .as_ref()
                        .is_none_or(|o| i["organizationId"].as_str() == Some(o))
                })
                .collect();
            println!("{}", Value::Array(items));
            ExitCode::SUCCESS
        }
        ["sync" | "get" | "list", ..] => fail("You are not logged in."),
        _ => fail(&format!(
            "fake bw: unsupported invocation: {}",
            args.join(" ")
        )),
    }
}
