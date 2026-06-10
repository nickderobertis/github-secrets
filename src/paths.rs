use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

const HOME_ENV: &str = "GH_SECRETS_HOME";

/// Resolves on-disk locations for the app config and per-profile config files.
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

    pub fn app_file(&self) -> PathBuf {
        self.root.join("app.json")
    }

    pub fn profiles_dir(&self) -> PathBuf {
        self.root.join("profiles")
    }

    pub fn profile_file(&self, name: &str) -> PathBuf {
        self.profiles_dir().join(format!("{name}.json"))
    }
}
