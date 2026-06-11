//! Repo-local manifest config + sync-state types.
//!
//! Two files live next to each other (typically at the repo root):
//!
//! - `gh-secrets.json` — checked-in manifest: source, list of secrets to
//!   manage, list of destinations to push to.
//! - `.gh-secrets-state.json` — gitignored, stores per-secret, per-destination
//!   SHA-256 hashes so we can do "push only when something changed" without
//!   ever persisting the plaintext value.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const DEFAULT_MANIFEST_FILE: &str = "gh-secrets.json";
pub const DEFAULT_STATE_FILE: &str = ".gh-secrets-state.json";

/// One secret managed by the manifest. `item` defaults to `name` when omitted
/// (i.e. the source's identifier matches the secret name). `field` defaults to
/// the source's own default (typically `password` for Bitwarden logins).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestSecret {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

impl ManifestSecret {
    /// The identifier passed to the source for lookup.
    pub fn source_item(&self) -> &str {
        self.item.as_deref().unwrap_or(&self.name)
    }
}

/// What kind of source the manifest pulls from. Tagged on `type` so we can add
/// further providers without breaking older configs.
///
/// Every store type declares a read/write capability: `bitwarden`, `env_file`,
/// and `local` are readable (sources); `github` is write-only and therefore
/// only appears in [`ManifestDestination`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ManifestSource {
    Bitwarden(BitwardenSourceConfig),
    /// A dotenv-style file: each managed secret's `item` (default: its name)
    /// is a key in the file.
    EnvFile(EnvFileSourceConfig),
    /// The global encrypted local store (`gh-secrets store`), shared across
    /// projects under the config root.
    Local,
}

impl ManifestSource {
    /// Short human label used in `list` output and error messages.
    pub fn label(&self) -> &'static str {
        match self {
            ManifestSource::Bitwarden(_) => "bitwarden",
            ManifestSource::EnvFile(_) => "env file",
            ManifestSource::Local => "local store",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvFileSourceConfig {
    /// Path to the env file, relative to the manifest's directory unless
    /// absolute.
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BitwardenSourceConfig {
    /// Optional collection ID to scope lookups to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection_id: Option<String>,
    /// Optional organization ID to scope lookups to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<String>,
    /// Default field to extract when a secret doesn't specify one. Defaults to
    /// `password` (matching a Bitwarden Login item).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_field: Option<String>,
}

/// One sync destination. Tagged on `type` for forward-compat.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ManifestDestination {
    Github(GithubDestinationConfig),
    EnvFile(EnvFileDestinationConfig),
    /// The global encrypted local store — the write half of
    /// [`ManifestSource::Local`].
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GithubDestinationConfig {
    /// `owner/repo`.
    pub repository: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvFileDestinationConfig {
    /// Path to the env file, relative to the manifest's directory unless
    /// absolute.
    pub path: PathBuf,
}

impl ManifestDestination {
    /// Stable identifier used as a key in `SyncState` so adding/removing
    /// destinations doesn't reset every secret's state.
    pub fn key(&self) -> String {
        match self {
            ManifestDestination::Github(c) => format!("github:{}", c.repository),
            ManifestDestination::EnvFile(c) => format!("env_file:{}", c.path.display()),
            ManifestDestination::Local => "local".to_string(),
        }
    }
}

/// The full config: a source, the managed secrets, and the destinations. The
/// same schema serves the project-local `gh-secrets.json` (checked in) and the
/// global config under the config root.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub source: ManifestSource,
    #[serde(default)]
    pub secrets: Vec<ManifestSecret>,
    #[serde(default)]
    pub destinations: Vec<ManifestDestination>,
}

impl Manifest {
    pub fn starter() -> Self {
        Self {
            source: ManifestSource::Bitwarden(BitwardenSourceConfig::default()),
            secrets: vec![ManifestSecret {
                name: "EXAMPLE_SECRET".into(),
                item: Some("example-bitwarden-item-name-or-id".into()),
                field: None,
            }],
            destinations: vec![
                ManifestDestination::Github(GithubDestinationConfig {
                    repository: "owner/repo".into(),
                }),
                ManifestDestination::EnvFile(EnvFileDestinationConfig {
                    path: PathBuf::from(".env"),
                }),
            ],
        }
    }

    /// Starter for the global config: secrets live in the encrypted local
    /// store and sync to explicitly listed repositories.
    pub fn global_starter() -> Self {
        Self {
            source: ManifestSource::Local,
            secrets: vec![ManifestSecret {
                name: "EXAMPLE_SECRET".into(),
                item: None,
                field: None,
            }],
            destinations: vec![ManifestDestination::Github(GithubDestinationConfig {
                repository: "owner/repo".into(),
            })],
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(self).context("serializing manifest")?;
        write_atomic(path, &bytes)
    }
}

/// Sync state co-located with the manifest. Keyed `secret_name -> destination
/// key -> SHA-256 of the last value we successfully pushed there`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncState {
    #[serde(default)]
    pub secrets: BTreeMap<String, SecretSyncState>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecretSyncState {
    /// Hash of the last value we observed from the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hash: Option<String>,
    /// Per-destination hash of the last value we successfully pushed.
    #[serde(default)]
    pub destinations: BTreeMap<String, String>,
}

impl SyncState {
    pub fn load_or_default(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(self).context("serializing sync state")?;
        write_atomic(path, &bytes)
    }

    pub fn last_pushed_hash(&self, name: &str, destination_key: &str) -> Option<&str> {
        self.secrets
            .get(name)
            .and_then(|s| s.destinations.get(destination_key))
            .map(String::as_str)
    }

    pub fn record_push(&mut self, name: &str, destination_key: &str, hash: &str) {
        let entry = self.secrets.entry(name.to_string()).or_default();
        entry
            .destinations
            .insert(destination_key.to_string(), hash.to_string());
    }

    pub fn record_source(&mut self, name: &str, hash: &str) {
        let entry = self.secrets.entry(name.to_string()).or_default();
        entry.source_hash = Some(hash.to_string());
    }
}

/// SHA-256 of `name + NUL + value`, hex-encoded. Domain-separated so renaming a
/// secret never reuses a stale hash from a different name.
pub fn value_hash(name: &str, value: &str) -> String {
    let mut h = Sha256::new();
    h.update(name.as_bytes());
    h.update([0u8]);
    h.update(value.as_bytes());
    let digest = h.finalize();
    hex_lower(&digest)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    let tmp = match path.extension().and_then(|s| s.to_str()) {
        Some(ext) => path.with_extension(format!("{ext}.tmp")),
        None => path.with_extension("tmp"),
    };
    fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trip_json() {
        let m = Manifest::starter();
        let s = serde_json::to_string_pretty(&m).unwrap();
        let back: Manifest = serde_json::from_str(&s).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn env_file_and_local_source_round_trip_json() {
        let m = Manifest {
            source: ManifestSource::EnvFile(EnvFileSourceConfig {
                path: PathBuf::from(".env.master"),
            }),
            secrets: vec![],
            destinations: vec![ManifestDestination::Local],
        };
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains(r#""type":"env_file""#));
        assert!(s.contains(r#""type":"local""#));
        let back: Manifest = serde_json::from_str(&s).unwrap();
        assert_eq!(m, back);
        let g = serde_json::to_string(&Manifest::global_starter()).unwrap();
        let back: Manifest = serde_json::from_str(&g).unwrap();
        assert_eq!(back.source, ManifestSource::Local);
    }

    #[test]
    fn destination_key_is_stable_and_unique() {
        let a = ManifestDestination::Github(GithubDestinationConfig {
            repository: "o/r".into(),
        });
        let b = ManifestDestination::EnvFile(EnvFileDestinationConfig {
            path: PathBuf::from(".env"),
        });
        assert_eq!(a.key(), "github:o/r");
        assert_eq!(b.key(), "env_file:.env");
        assert_ne!(a.key(), b.key());
    }

    #[test]
    fn source_item_defaults_to_name() {
        let s = ManifestSecret {
            name: "FOO".into(),
            item: None,
            field: None,
        };
        assert_eq!(s.source_item(), "FOO");
        let s2 = ManifestSecret {
            name: "FOO".into(),
            item: Some("foo-bw".into()),
            field: None,
        };
        assert_eq!(s2.source_item(), "foo-bw");
    }

    #[test]
    fn value_hash_is_domain_separated() {
        assert_ne!(value_hash("A", "BC"), value_hash("AB", "C"));
        assert_eq!(value_hash("X", "Y"), value_hash("X", "Y"));
    }

    #[test]
    fn sync_state_round_trip() {
        let mut s = SyncState::default();
        s.record_source("FOO", "deadbeef");
        s.record_push("FOO", "github:o/r", "cafebabe");
        let bytes = serde_json::to_vec(&s).unwrap();
        let back: SyncState = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(s, back);
        assert_eq!(back.last_pushed_hash("FOO", "github:o/r"), Some("cafebabe"));
        assert_eq!(back.last_pushed_hash("FOO", "env_file:.env"), None);
    }
}
