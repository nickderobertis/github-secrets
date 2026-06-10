use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths::Paths;
use crate::secrets::{GlobalSecrets, RepositorySecrets, SyncRecord};

pub const DEFAULT_PROFILE: &str = "default";

/// App-level state: which profile is active and the list of known profiles.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppConfig {
    pub current_profile: String,
    pub profiles: Vec<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            current_profile: DEFAULT_PROFILE.to_string(),
            profiles: vec![DEFAULT_PROFILE.to_string()],
        }
    }
}

impl AppConfig {
    pub fn load_or_default(path: &Path) -> Result<Self> {
        load_json_or_default(path)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        save_json(path, self)
    }

    pub fn has_profile(&self, name: &str) -> bool {
        self.profiles.iter().any(|n| n == name)
    }

    pub fn add_profile(&mut self, name: &str) {
        if !self.has_profile(name) {
            self.profiles.push(name.to_string());
        }
    }

    pub fn remove_profile(&mut self, name: &str) {
        self.profiles.retain(|n| n != name);
    }
}

/// All state for a single profile: which repositories are in/out, the secrets,
/// and the per-(repo, secret) sync records.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileConfig {
    #[serde(default)]
    pub github_token: String,
    #[serde(default)]
    pub include_repositories: Option<Vec<String>>,
    #[serde(default)]
    pub exclude_repositories: Option<Vec<String>>,
    #[serde(default)]
    pub global_secrets: GlobalSecrets,
    #[serde(default)]
    pub repository_secrets: RepositorySecrets,
    #[serde(default)]
    pub sync_records: BTreeMap<String, Vec<SyncRecord>>,
}

impl ProfileConfig {
    pub fn load_or_default(path: &Path) -> Result<Self> {
        load_json_or_default(path)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        save_json(path, self)
    }

    pub fn add_include(&mut self, repo: &str) -> Result<()> {
        if let Some(excluded) = &self.exclude_repositories {
            if excluded.iter().any(|r| r == repo) {
                anyhow::bail!("{repo} is in the excluded list; remove it from excluded first");
            }
        }
        let list = self.include_repositories.get_or_insert_with(Vec::new);
        if list.iter().any(|r| r == repo) {
            anyhow::bail!("{repo} is already included");
        }
        list.push(repo.to_string());
        Ok(())
    }

    pub fn remove_include(&mut self, repo: &str) -> Result<()> {
        let list = self
            .include_repositories
            .as_mut()
            .context("no included repositories")?;
        if !list.iter().any(|r| r == repo) {
            anyhow::bail!("{repo} is not in the included list");
        }
        list.retain(|r| r != repo);
        Ok(())
    }

    pub fn add_exclude(&mut self, repo: &str) -> Result<()> {
        if let Some(included) = &self.include_repositories {
            if included.iter().any(|r| r == repo) {
                anyhow::bail!("{repo} is in the included list; remove it from included first");
            }
        }
        let list = self.exclude_repositories.get_or_insert_with(Vec::new);
        if list.iter().any(|r| r == repo) {
            anyhow::bail!("{repo} is already excluded");
        }
        list.push(repo.to_string());
        Ok(())
    }

    pub fn remove_exclude(&mut self, repo: &str) -> Result<()> {
        let list = self
            .exclude_repositories
            .as_mut()
            .context("no excluded repositories")?;
        if !list.iter().any(|r| r == repo) {
            anyhow::bail!("{repo} is not in the excluded list");
        }
        list.retain(|r| r != repo);
        Ok(())
    }
}

/// Convenience: load the active profile.
pub fn load_active_profile(paths: &Paths) -> Result<(AppConfig, ProfileConfig)> {
    let app = AppConfig::load_or_default(&paths.app_file())?;
    let profile = ProfileConfig::load_or_default(&paths.profile_file(&app.current_profile))?;
    Ok((app, profile))
}

fn load_json_or_default<T: serde::de::DeserializeOwned + Default>(path: &Path) -> Result<T> {
    if !path.exists() {
        return Ok(T::default());
    }
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
}

fn save_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(value).context("serializing config")?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}
