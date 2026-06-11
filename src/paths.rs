use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

const HOME_ENV: &str = "GH_SECRETS_HOME";

/// Resolves on-disk locations under the config root: the encrypted vault and
/// the global config/state pair.
///
/// Honors `GH_SECRETS_HOME` when set (used by tests to redirect at a tempdir)
/// and otherwise falls back to the platform config directory.
#[derive(Debug, Clone)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    pub fn resolve() -> Result<Self> {
        if let Some(env) = std::env::var_os(HOME_ENV) {
            return Ok(Self {
                root: PathBuf::from(env),
            });
        }
        let cfg = dirs::config_dir()
            .ok_or_else(|| anyhow!("no platform config directory; set {HOME_ENV}"))?;
        Ok(Self {
            root: cfg.join("gh-secrets"),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The encrypted vault: stored credentials (`gh-secrets auth`) plus the
    /// global `local` store's secret values (`gh-secrets store`).
    pub fn vault_file(&self) -> PathBuf {
        self.root.join("vault.json")
    }

    /// The global config, used when the working directory has no
    /// `gh-secrets.json` (or when `--global` forces it). Same schema as the
    /// project-local manifest.
    pub fn global_manifest_file(&self) -> PathBuf {
        self.root.join("gh-secrets.json")
    }

    /// Sync state for the global config.
    pub fn global_state_file(&self) -> PathBuf {
        self.root.join(".gh-secrets-state.json")
    }
}
