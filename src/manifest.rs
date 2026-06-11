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

/// One secret managed by the manifest.
///
/// `name` is the secret's identity on the *source* side: it's the lookup key
/// for the source (unless `item` overrides it) and the key the sync state hashes
/// against. `field` defaults to the source's own default (typically `password`
/// for Bitwarden logins).
///
/// `destination_names` is the identity on the *destination* side: the names this
/// value is written under at every destination. When omitted it defaults to
/// `[name]`, so the common "same name everywhere" case stays a single `name`.
/// Supplying it lets the destination name differ from the source identity, and
/// lets one source value fan out to several destination names (e.g. a single
/// publish token written as both `NPM_TOKEN` and `NODE_AUTH_TOKEN`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestSecret {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub destination_names: Vec<String>,
}

impl ManifestSecret {
    /// The identifier passed to the source for lookup.
    pub fn source_item(&self) -> &str {
        self.item.as_deref().unwrap_or(&self.name)
    }

    /// The names this value is written under at each destination. Defaults to
    /// `[name]` when `destination_names` is omitted, so a secret that keeps the
    /// same name everywhere needs no extra config.
    pub fn dest_names(&self) -> Vec<&str> {
        if self.destination_names.is_empty() {
            vec![self.name.as_str()]
        } else {
            self.destination_names.iter().map(String::as_str).collect()
        }
    }
}

/// What kind of source the manifest pulls from. Tagged on `type` so we can add
/// further providers without breaking older configs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ManifestSource {
    Bitwarden(BitwardenSourceConfig),
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
        }
    }
}

/// The full manifest, loaded from `gh-secrets.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoManifest {
    pub source: ManifestSource,
    #[serde(default)]
    pub secrets: Vec<ManifestSecret>,
    #[serde(default)]
    pub destinations: Vec<ManifestDestination>,
}

impl RepoManifest {
    pub fn starter() -> Self {
        Self {
            source: ManifestSource::Bitwarden(BitwardenSourceConfig::default()),
            secrets: vec![ManifestSecret {
                name: "EXAMPLE_SECRET".into(),
                item: Some("example-bitwarden-item-name-or-id".into()),
                field: None,
                destination_names: Vec::new(),
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
        let m = RepoManifest::starter();
        let s = serde_json::to_string_pretty(&m).unwrap();
        let back: RepoManifest = serde_json::from_str(&s).unwrap();
        assert_eq!(m, back);
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
            destination_names: Vec::new(),
        };
        assert_eq!(s.source_item(), "FOO");
        let s2 = ManifestSecret {
            name: "FOO".into(),
            item: Some("foo-bw".into()),
            field: None,
            destination_names: Vec::new(),
        };
        assert_eq!(s2.source_item(), "foo-bw");
    }

    #[test]
    fn dest_names_defaults_to_name_then_uses_overrides() {
        // Omitted: the secret keeps its single source-side name on the
        // destination too.
        let single = ManifestSecret {
            name: "FOO".into(),
            item: None,
            field: None,
            destination_names: Vec::new(),
        };
        assert_eq!(single.dest_names(), vec!["FOO"]);

        // Supplied: the value fans out to every listed destination name, which
        // can differ entirely from the source-side `name`.
        let fanned = ManifestSecret {
            name: "npm-publish-token".into(),
            item: None,
            field: None,
            destination_names: vec!["NPM_TOKEN".into(), "NODE_AUTH_TOKEN".into()],
        };
        assert_eq!(fanned.dest_names(), vec!["NPM_TOKEN", "NODE_AUTH_TOKEN"]);
    }

    #[test]
    fn destination_names_omitted_from_json_when_empty() {
        let s = ManifestSecret {
            name: "FOO".into(),
            item: None,
            field: None,
            destination_names: Vec::new(),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("destination_names"), "got: {json}");
        // And it round-trips back to an empty list (defaulted).
        let back: ManifestSecret = serde_json::from_str(&json).unwrap();
        assert_eq!(back.destination_names, Vec::<String>::new());
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
