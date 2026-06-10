//! Stored credentials for the manifest flow + the resolution that layers the
//! environment over them.
//!
//! Precedence (highest first): shell env, then `.env`, then `.env.local` (all
//! three already collapsed into the process environment by
//! [`crate::envfile::load_dotenv_cwd`]), then this stored config file. The
//! resolver here implements only the final `env → stored` step; the dotenv
//! layering happens before it runs.
//!
//! The file lives at `<config-root>/credentials.json` (profile-independent,
//! because `manifest sync` is independent of the active profile) and is written
//! `0600` on Unix. It is the *only* place — besides the env vars — a user opts
//! into persisting these credentials; treat its contents as sensitive.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::destinations::github_token_from_env;
use crate::sources::BitwardenCredentials;

/// On-disk credential store. Every field is optional so a user can persist only
/// what they want (e.g. just the Bitwarden API key, keeping the GitHub token in
/// CI env).
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
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(self).context("serializing credentials")?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, &bytes).with_context(|| format!("writing {}", tmp.display()))?;
        set_owner_only(&tmp)?;
        fs::rename(&tmp, path)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.github_token.is_none() && self.bitwarden.is_empty()
    }
}

#[cfg(unix)]
fn set_owner_only(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("setting 0600 perms on {}", path.display()))
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> Result<()> {
    Ok(())
}

/// Fully resolved manifest credentials: the environment (incl. loaded dotenv)
/// over stored config.
#[derive(Debug, Clone, Default)]
pub struct ResolvedCredentials {
    pub github_token: Option<String>,
    pub bitwarden: BitwardenCredentials,
}

impl ResolvedCredentials {
    pub fn resolve(stored: &StoredCredentials) -> Self {
        Self {
            github_token: github_token_from_env()
                .or_else(|| stored.github_token.clone().filter(|s| !s.is_empty())),
            bitwarden: BitwardenCredentials::from_env().or_stored(
                stored.bitwarden.client_id.as_deref(),
                stored.bitwarden.client_secret.as_deref(),
                stored.bitwarden.master_password.as_deref(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn round_trips_and_omits_empty_sections() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("credentials.json");
        let creds = StoredCredentials {
            github_token: Some("ghp_x".into()),
            ..Default::default()
        };
        creds.save(&path).unwrap();
        // The empty bitwarden section is skipped entirely.
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("github_token"));
        assert!(!raw.contains("bitwarden"));
        let back = StoredCredentials::load(&path).unwrap();
        assert_eq!(creds, back);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn missing_file_loads_as_default() {
        let dir = TempDir::new().unwrap();
        let got = StoredCredentials::load(&dir.path().join("nope.json")).unwrap();
        assert_eq!(got, StoredCredentials::default());
        assert!(got.is_empty());
    }

    #[test]
    fn resolve_falls_back_to_stored_when_env_absent() {
        // Ensure the env doesn't accidentally provide these (nextest isolates
        // each test in its own process, so this is safe).
        for k in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "BW_CLIENTID",
            "BITWARDEN_CLIENT_ID",
            "BW_CLIENTSECRET",
            "BITWARDEN_CLIENT_SECRET",
            "BW_PASSWORD",
            "BITWARDEN_MASTER_PASSWORD",
            "BITWARDEN_PASSWORD",
            "BW_SESSION",
            "BITWARDEN_SESSION",
        ] {
            std::env::remove_var(k);
        }
        let stored = StoredCredentials {
            github_token: Some("stored-gh".into()),
            bitwarden: StoredBitwarden {
                client_id: Some("stored-id".into()),
                client_secret: Some("stored-secret".into()),
                master_password: Some("stored-pw".into()),
            },
        };
        let resolved = ResolvedCredentials::resolve(&stored);
        assert_eq!(resolved.github_token.as_deref(), Some("stored-gh"));
        assert_eq!(resolved.bitwarden.client_id.as_deref(), Some("stored-id"));
        assert_eq!(resolved.bitwarden.password.as_deref(), Some("stored-pw"));

        // Now the environment must win over stored config.
        std::env::set_var("GH_TOKEN", "env-gh");
        std::env::set_var("BW_PASSWORD", "env-pw");
        let resolved = ResolvedCredentials::resolve(&stored);
        assert_eq!(resolved.github_token.as_deref(), Some("env-gh"));
        assert_eq!(resolved.bitwarden.password.as_deref(), Some("env-pw"));
        // The fields the env didn't set still come from stored config.
        assert_eq!(resolved.bitwarden.client_id.as_deref(), Some("stored-id"));
        std::env::remove_var("GH_TOKEN");
        std::env::remove_var("BW_PASSWORD");
    }
}
