//! Stored credentials for `gh-secrets auth`.
//!
//! Precedence (highest first): shell env, then `.env`, then `.env.local` (all
//! three collapsed into the process environment by
//! [`crate::envfile::load_dotenv_cwd`]), then this stored config — the final
//! `env → stored` step is implemented by `crate::engine::LazyCredentials`,
//! which decrypts the vault only when a stored value is actually needed.
//!
//! Storage lives inside the encrypted vault (`crate::vault`), written by
//! `gh-secrets auth ...`. It is the *only* place — besides the env vars — a
//! user opts into persisting these credentials.

use serde::{Deserialize, Serialize};

/// Stored credential set inside the vault. Every field is optional so a user
/// can persist only what they want (e.g. just the Bitwarden API key, keeping
/// the GitHub token in CI env).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredCredentials {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_token: Option<String>,
    #[serde(default, skip_serializing_if = "StoredBitwarden::is_empty")]
    pub bitwarden: StoredBitwarden,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredBitwarden {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master_password: Option<String>,
}

impl StoredBitwarden {
    pub fn is_empty(&self) -> bool {
        self.client_id.is_none() && self.client_secret.is_none() && self.master_password.is_none()
    }
}

impl StoredCredentials {
    pub fn is_empty(&self) -> bool {
        self.github_token.is_none() && self.bitwarden.is_empty()
    }
}
