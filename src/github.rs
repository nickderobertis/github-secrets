use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use reqwest::StatusCode;
use serde::Deserialize;

const DEFAULT_API_BASE: &str = "https://api.github.com";
const API_BASE_ENV: &str = "GH_SECRETS_API_BASE";

/// Minimal typed GitHub REST client covering only the endpoints `gh-secrets`
/// actually uses.
pub struct GitHubClient {
    base: String,
    http: Client,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepoPublicKey {
    pub key_id: String,
    pub key: String,
}

#[derive(Debug, Deserialize)]
struct RepoSummary {
    full_name: String,
}

impl GitHubClient {
    pub fn new(token: &str) -> Result<Self> {
        if token.is_empty() {
            bail!("GitHub token is not set; run `gh-secrets token <token>` first");
        }
        let base = std::env::var(API_BASE_ENV).unwrap_or_else(|_| DEFAULT_API_BASE.to_string());
        let mut headers = HeaderMap::new();
        let mut auth =
            HeaderValue::from_str(&format!("Bearer {token}")).context("building auth header")?;
        auth.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth);
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(USER_AGENT, HeaderValue::from_static("gh-secrets"));
        let http = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()
            .context("building HTTP client")?;
        Ok(Self { base, http })
    }

    /// Lists every repository the authenticated user has access to (paginated).
    pub fn list_user_repositories(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let mut page: u32 = 1;
        loop {
            let url = format!("{}/user/repos?per_page=100&page={page}", self.base);
            let resp = self.http.get(&url).send().context("calling /user/repos")?;
            let status = resp.status();
            let body = resp.text().context("reading /user/repos body")?;
            if !status.is_success() {
                bail!(github_error_message(status, "GET /user/repos", &body));
            }
            let repos: Vec<RepoSummary> = serde_json::from_str(&body)
                .with_context(|| format!("parsing /user/repos page {page}"))?;
            if repos.is_empty() {
                break;
            }
            let count = repos.len();
            out.extend(repos.into_iter().map(|r| r.full_name));
            if count < 100 {
                break;
            }
            page += 1;
        }
        Ok(out)
    }

    pub fn get_public_key(&self, repo: &str) -> Result<RepoPublicKey> {
        let url = format!("{}/repos/{repo}/actions/secrets/public-key", self.base);
        let resp = self.http.get(&url).send().context("calling public-key")?;
        let status = resp.status();
        let body = resp.text().context("reading public-key body")?;
        if !status.is_success() {
            bail!(github_error_message(
                status,
                &format!("GET public-key for {repo}"),
                &body
            ));
        }
        serde_json::from_str(&body).with_context(|| format!("parsing public-key for {repo}"))
    }

    /// PUTs an encrypted secret. Returns `true` if the secret was newly
    /// created (201) and `false` if it was updated (204).
    pub fn put_secret(
        &self,
        repo: &str,
        name: &str,
        encrypted_value_b64: &str,
        key_id: &str,
    ) -> Result<bool> {
        let url = format!("{}/repos/{repo}/actions/secrets/{name}", self.base);
        let body = serde_json::json!({
            "encrypted_value": encrypted_value_b64,
            "key_id": key_id,
        });
        let resp = self
            .http
            .put(&url)
            .json(&body)
            .send()
            .context("calling PUT secret")?;
        let status = resp.status();
        match status {
            StatusCode::CREATED => Ok(true),
            StatusCode::NO_CONTENT => Ok(false),
            _ => {
                let body = resp.text().unwrap_or_default();
                Err(anyhow!(github_error_message(
                    status,
                    &format!("PUT secret {name} for {repo}"),
                    &body
                )))
            }
        }
    }
}

fn github_error_message(status: StatusCode, op: &str, body: &str) -> String {
    let msg = match status {
        StatusCode::UNAUTHORIZED => {
            "GitHub returned 401 Unauthorized — set a valid token with `gh-secrets token <token>`"
        }
        StatusCode::FORBIDDEN => {
            "GitHub returned 403 Forbidden — the token lacks the required permissions (need `repo` scope or fine-grained `secrets:write`)"
        }
        StatusCode::NOT_FOUND => {
            "GitHub returned 404 Not Found — check the repository name or token access"
        }
        _ => "GitHub request failed",
    };
    let trimmed = body.trim();
    if trimmed.is_empty() {
        format!("{op}: {status} — {msg}")
    } else {
        format!("{op}: {status} — {msg}\n  body: {trimmed}")
    }
}
