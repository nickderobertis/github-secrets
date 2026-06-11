use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
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

impl GitHubClient {
    pub fn new(token: &str) -> Result<Self> {
        if token.is_empty() {
            bail!("GitHub token is not set; set GH_TOKEN or run `gh-secrets auth github <token>`");
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

/// Encrypts `plaintext` for the given base64-encoded X25519 public key using
/// libsodium-compatible sealed-box semantics, and returns the ciphertext as
/// base64. Matches what GitHub expects for `actions/secrets`.
pub fn seal(public_key_b64: &str, plaintext: &str) -> Result<String> {
    let key_bytes = B64.decode(public_key_b64).context("decoding public key")?;
    let key_array: [u8; 32] = key_bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("public key has wrong length (expected 32 bytes)"))?;
    let pk = crypto_box::PublicKey::from(key_array);
    let mut rng = crypto_box::aead::OsRng;
    let ciphertext = pk
        .seal(&mut rng, plaintext.as_bytes())
        .map_err(|_| anyhow!("sealing failed"))?;
    Ok(B64.encode(ciphertext))
}

fn github_error_message(status: StatusCode, op: &str, body: &str) -> String {
    let msg = match status {
        StatusCode::UNAUTHORIZED => {
            "GitHub returned 401 Unauthorized — set a valid token via GH_TOKEN or `gh-secrets auth github <token>`"
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_with_a_real_pubkey_roundtrips_lengthwise() {
        // 32-byte public key encoded in base64.
        let key = [7u8; 32];
        let b64 = B64.encode(key);
        let ct = seal(&b64, "hello").unwrap();
        let raw = B64.decode(ct).unwrap();
        // Sealed box overhead: ephemeral pubkey (32) + MAC (16) on top of msg.
        assert_eq!(raw.len(), 32 + 16 + "hello".len());
    }
}
