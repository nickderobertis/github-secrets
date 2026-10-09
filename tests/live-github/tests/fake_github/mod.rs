//! A stateful loopback double of the slice of the GitHub REST API the live
//! suite touches: repo lookup, the Actions public key, and secret
//! put/get/list/delete. It holds exactly one repo — the identity the test
//! configures — so a request for any other repo path gets a 404 and a suite
//! that ignores or miswires its configuration cannot pass against it. Every
//! request must carry `Authorization: Bearer <token>` or it gets GitHub's 401.
//! `mod fake_github` in `tests/sandbox_fixture.rs` pulls it in.

use std::collections::BTreeMap;
use std::sync::Mutex;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::json;
use tokio::runtime::Runtime;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// The public-key id the double serves and requires on every secret PUT.
pub const KEY_ID: &str = "fake-key-id";

struct State {
    repo: String,
    private: bool,
    token: String,
    /// name -> (created_at, updated_at)
    secrets: Mutex<BTreeMap<String, (String, String)>>,
    clock: Mutex<u32>,
}

impl State {
    fn tick(&self) -> String {
        let mut c = self.clock.lock().unwrap();
        *c += 1;
        // Zero-padded ISO-8601, so later instants compare greater as strings,
        // which is how the update journey reads GitHub's `updated_at`.
        format!(
            "2026-01-01T{:02}:{:02}:{:02}Z",
            *c / 3600,
            *c / 60 % 60,
            *c % 60
        )
    }

    fn respond(&self, req: &Request) -> ResponseTemplate {
        let authorized = req
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            == Some(format!("Bearer {}", self.token).as_str());
        if !authorized {
            return ResponseTemplate::new(401).set_body_json(json!({"message": "Bad credentials"}));
        }
        let method = req.method.as_str();
        let path = req.url.path();
        let repo_root = format!("/repos/{}", self.repo);
        let Some(rest) = path.strip_prefix(&repo_root) else {
            return not_found();
        };
        let mut secrets = self.secrets.lock().unwrap();
        match (method, rest) {
            ("GET", "") => ResponseTemplate::new(200).set_body_json(json!({
                "full_name": self.repo,
                "private": self.private,
            })),
            ("GET", "/actions/secrets/public-key") => {
                ResponseTemplate::new(200).set_body_json(json!({
                    "key_id": KEY_ID,
                    "key": B64.encode([7u8; 32]),
                }))
            }
            ("GET", "/actions/secrets") => {
                let list: Vec<_> = secrets.keys().map(|n| json!({ "name": n })).collect();
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "total_count": list.len(), "secrets": list }))
            }
            (m, r) if r.starts_with("/actions/secrets/") => {
                let name = r.trim_start_matches("/actions/secrets/").to_string();
                match m {
                    "GET" => match secrets.get(&name) {
                        Some((created, updated)) => {
                            ResponseTemplate::new(200).set_body_json(json!({
                                "name": name,
                                "created_at": created,
                                "updated_at": updated,
                            }))
                        }
                        None => not_found(),
                    },
                    "PUT" => {
                        if let Err(why) = check_put_body(&req.body) {
                            return ResponseTemplate::new(422)
                                .set_body_json(json!({ "message": why }));
                        }
                        let now = self.tick();
                        match secrets.get_mut(&name) {
                            Some(entry) => {
                                entry.1 = now;
                                ResponseTemplate::new(204)
                            }
                            None => {
                                secrets.insert(name, (now.clone(), now));
                                ResponseTemplate::new(201)
                            }
                        }
                    }
                    "DELETE" => match secrets.remove(&name) {
                        Some(_) => ResponseTemplate::new(204),
                        None => not_found(),
                    },
                    _ => not_found(),
                }
            }
            _ => not_found(),
        }
    }
}

/// GitHub's shape for a secret PUT: the double's `key_id` and a base64
/// sealed box (at least the 48-byte sealed-box overhead), so a binary sending
/// a malformed body fails here as it would against GitHub.
fn check_put_body(body: &[u8]) -> Result<(), String> {
    let v: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("body is not JSON: {e}"))?;
    if v["key_id"] != KEY_ID {
        return Err(format!("key_id must be {KEY_ID:?}"));
    }
    let sealed = v["encrypted_value"]
        .as_str()
        .ok_or("encrypted_value must be a string")?;
    let bytes = B64
        .decode(sealed)
        .map_err(|e| format!("encrypted_value is not base64: {e}"))?;
    if bytes.len() < 48 {
        return Err("encrypted_value is shorter than a sealed box".to_string());
    }
    Ok(())
}

fn not_found() -> ResponseTemplate {
    ResponseTemplate::new(404).set_body_json(json!({"message": "Not Found"}))
}

struct Responder(std::sync::Arc<State>);

impl Respond for Responder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        self.0.respond(req)
    }
}

/// The running double. Its multi-thread runtime serves requests in the
/// background, so the blocking helpers and the spawned binary can call it.
pub struct FakeGithub {
    rt: Runtime,
    server: MockServer,
}

impl FakeGithub {
    /// Serve one private repo (`owner/name`) to callers presenting `token`.
    pub fn start(repo: &str, token: &str) -> Self {
        Self::start_with(repo, token, true)
    }

    /// As [`FakeGithub::start`], choosing the repo's visibility.
    pub fn start_with(repo: &str, token: &str, private: bool) -> Self {
        let rt = Runtime::new().expect("tokio runtime for the GitHub double");
        let state = std::sync::Arc::new(State {
            repo: repo.to_string(),
            private,
            token: token.to_string(),
            secrets: Mutex::new(BTreeMap::new()),
            clock: Mutex::new(0),
        });
        let server = rt.block_on(async {
            let server = MockServer::start().await;
            Mock::given(wiremock::matchers::any())
                .respond_with(Responder(state))
                .mount(&server)
                .await;
            server
        });
        Self { rt, server }
    }

    pub fn uri(&self) -> String {
        self.server.uri()
    }

    /// `METHOD path` of every request the double received, in order.
    pub fn requests(&self) -> Vec<String> {
        self.rt
            .block_on(self.server.received_requests())
            .unwrap_or_default()
            .iter()
            .map(|r| format!("{} {}", r.method, r.url.path()))
            .collect()
    }
}
