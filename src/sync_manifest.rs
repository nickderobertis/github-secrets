//! Manifest-driven sync orchestrator: pull all managed secrets from the
//! configured source, push to every destination that's missing the current
//! value, and persist per-destination hashes for next time.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;

use anyhow::{anyhow, Context, Result};

use crate::credentials::{ResolvedCredentials, StoredCredentials};
use crate::destinations::{
    Destination, DestinationEntry, DestinationReport, DestinationRequest, EnvFileDestination,
    GitHubDestination,
};
use crate::manifest::{
    value_hash, ManifestDestination, ManifestSource, RepoManifest, SyncState, DEFAULT_STATE_FILE,
};
use crate::paths::Paths;
use crate::sources::{
    BitwardenCredentials, BitwardenSource, SecretSource, SourceItem, StaticSource,
};

/// Test-only override: if set, points at a JSON file `{ "NAME": "value", ... }`
/// that is used as a static source in place of the manifest's configured one.
/// Lets the wiremock e2e suite drive the binary without contacting Bitwarden.
/// Intentionally undocumented in `--help`, mirroring `GH_SECRETS_API_BASE`.
pub const TEST_SOURCE_FILE_ENV: &str = "GH_SECRETS_TEST_SOURCE_FILE";

#[derive(Debug, Default)]
pub struct ManifestSyncReport {
    pub destinations: Vec<DestinationOutcome>,
}

#[derive(Debug)]
pub struct DestinationOutcome {
    pub destination_key: String,
    pub report: DestinationReport,
}

impl ManifestSyncReport {
    pub fn is_noop(&self) -> bool {
        self.destinations.iter().all(|d| !d.report.changed())
    }
}

/// Top-level entry: load manifest + state, pull from source, push to all
/// destinations, write state. `manifest_path` and `state_path` are absolute or
/// relative to the caller's CWD.
pub fn sync_manifest(
    manifest_path: &Path,
    state_path: Option<&Path>,
) -> Result<ManifestSyncReport> {
    let manifest = RepoManifest::load(manifest_path)
        .with_context(|| format!("loading manifest from {}", manifest_path.display()))?;
    let state_path = match state_path {
        Some(p) => p.to_path_buf(),
        None => default_state_path(manifest_path),
    };
    let mut state = SyncState::load_or_default(&state_path)?;
    // Resolve credentials once: process env (which the caller has already
    // populated from `.env`/`.env.local`) layered over the stored config file.
    let stored = load_stored_credentials()?;
    let resolved = ResolvedCredentials::resolve(&stored);
    let source = resolve_source(&manifest.source, &resolved.bitwarden)?;
    let report = run(
        &manifest,
        &mut state,
        source.as_ref(),
        &resolved,
        manifest_path,
    )?;
    state.save(&state_path)?;
    Ok(report)
}

/// Enumerate the items available in the manifest's source (e.g. the Bitwarden
/// vault, scoped by the manifest's collection/organization), so a user can
/// discover which item names exist to reference from `gh-secrets.json`. Reads
/// only identity metadata — never a secret value. Resolves credentials the same
/// way `sync_manifest` does (env over stored config), and unlocks the source.
pub fn list_source_items(manifest_path: &Path) -> Result<Vec<SourceItem>> {
    let manifest = RepoManifest::load(manifest_path)
        .with_context(|| format!("loading manifest from {}", manifest_path.display()))?;
    let stored = load_stored_credentials()?;
    let resolved = ResolvedCredentials::resolve(&stored);
    let source = resolve_source(&manifest.source, &resolved.bitwarden)?;
    source.list_available()
}

/// Load the stored credential config from the resolved config root. Honors
/// `GH_SECRETS_HOME` (so tests stay isolated) just like the profile config.
fn load_stored_credentials() -> Result<StoredCredentials> {
    let paths = Paths::resolve()?;
    StoredCredentials::load(&paths.credentials_file())
}

fn resolve_source(
    source: &ManifestSource,
    bitwarden: &BitwardenCredentials,
) -> Result<Box<dyn SecretSource>> {
    if let Ok(path) = env::var(TEST_SOURCE_FILE_ENV) {
        let bytes = fs::read(&path).with_context(|| format!("reading test source file {path}"))?;
        let values: HashMap<String, String> = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing test source file {path} as JSON"))?;
        return Ok(Box::new(StaticSource { values }));
    }
    match source {
        ManifestSource::Bitwarden(cfg) => Ok(Box::new(BitwardenSource::with_credentials(
            cfg.clone(),
            bitwarden.clone(),
        ))),
    }
}

/// Same as `sync_manifest` but takes a pre-built source — used by tests with a
/// `StaticSource`.
pub fn sync_manifest_with_source(
    manifest_path: &Path,
    state_path: Option<&Path>,
    source: &dyn SecretSource,
) -> Result<ManifestSyncReport> {
    let manifest = RepoManifest::load(manifest_path)
        .with_context(|| format!("loading manifest from {}", manifest_path.display()))?;
    let state_path = match state_path {
        Some(p) => p.to_path_buf(),
        None => default_state_path(manifest_path),
    };
    let mut state = SyncState::load_or_default(&state_path)?;
    // A caller-supplied source still pushes to manifest destinations, so the
    // GitHub destination needs a resolved token. This helper exists for tests,
    // so resolve from the environment only (no config-file read) to stay
    // hermetic; Bitwarden creds are unused because the source is provided.
    let resolved = ResolvedCredentials::resolve(&StoredCredentials::default());
    let report = run(&manifest, &mut state, source, &resolved, manifest_path)?;
    state.save(&state_path)?;
    Ok(report)
}

fn run(
    manifest: &RepoManifest,
    state: &mut SyncState,
    source: &dyn SecretSource,
    resolved: &ResolvedCredentials,
    manifest_path: &Path,
) -> Result<ManifestSyncReport> {
    let base_dir = manifest_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let fetched = source
        .fetch(&manifest.secrets)
        .context("fetching from source")?;
    // The source returns one value per managed secret, keyed by the secret's
    // (source-side) `name`; index it so we can fan each value out to its
    // destination names regardless of the order the source returned them in.
    let fetched_by_name: HashMap<&str, &str> = fetched
        .iter()
        .map(|f| (f.name.as_str(), f.value.as_str()))
        .collect();

    // Fan each managed secret out into one destination entry per destination
    // name. The source hash is recorded once under the secret's `name`; each
    // pushed entry hashes against its own destination name (so the same value
    // written under two names tracks independently in the state file).
    let mut entries: Vec<DestinationEntry> = Vec::with_capacity(fetched.len());
    for secret in &manifest.secrets {
        let value = *fetched_by_name.get(secret.name.as_str()).ok_or_else(|| {
            anyhow!(
                "source returned no value for managed secret '{}'",
                secret.name
            )
        })?;
        state.record_source(&secret.name, &value_hash(&secret.name, value));
        for dest_name in secret.dest_names() {
            entries.push(DestinationEntry {
                name: dest_name.to_string(),
                value: value.to_string(),
                current_hash: value_hash(dest_name, value),
                last_pushed_hash: None, // filled in per-destination below
            });
        }
    }

    let mut report = ManifestSyncReport::default();
    for dest_cfg in &manifest.destinations {
        let mut destination: Box<dyn Destination> = match dest_cfg {
            ManifestDestination::Github(c) => {
                let token = resolved.github_token.as_deref().ok_or_else(|| {
                    anyhow!(
                        "no GitHub token for destination github:{}: set GH_TOKEN/GITHUB_TOKEN (e.g. in .env) or run `gh-secrets auth github <token>`",
                        c.repository
                    )
                })?;
                Box::new(GitHubDestination::from_config(c, token)?)
            }
            ManifestDestination::EnvFile(c) => {
                Box::new(EnvFileDestination::from_config(c, base_dir))
            }
        };
        let dest_key = destination.key();
        // Per-destination request: inject the last-pushed hash from state.
        let mut req = DestinationRequest::default();
        for entry in &entries {
            let last = state
                .last_pushed_hash(&entry.name, &dest_key)
                .map(String::from);
            req.entries.push(DestinationEntry {
                name: entry.name.clone(),
                value: entry.value.clone(),
                current_hash: entry.current_hash.clone(),
                last_pushed_hash: last,
            });
        }
        let dest_report = destination
            .apply(req)
            .with_context(|| format!("applying to destination {dest_key}"))?;
        for name in dest_report.created.iter().chain(dest_report.updated.iter()) {
            // Find the hash we just pushed.
            if let Some(entry) = entries.iter().find(|e| &e.name == name) {
                state.record_push(name, &dest_key, &entry.current_hash);
            }
        }
        // For unchanged destinations whose state was missing (e.g. first run
        // against a file that already had the value), still record so we
        // converge on the no-op next time.
        for name in &dest_report.unchanged {
            if let Some(entry) = entries.iter().find(|e| &e.name == name) {
                state.record_push(name, &dest_key, &entry.current_hash);
            }
        }
        report.destinations.push(DestinationOutcome {
            destination_key: dest_key,
            report: dest_report,
        });
    }
    Ok(report)
}

fn default_state_path(manifest_path: &Path) -> std::path::PathBuf {
    manifest_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join(DEFAULT_STATE_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{BitwardenSourceConfig, EnvFileDestinationConfig, ManifestSecret};
    use crate::sources::StaticSource;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn manifest_with_env_file(dir: &Path, env_path: PathBuf) -> PathBuf {
        let manifest = RepoManifest {
            source: ManifestSource::Bitwarden(BitwardenSourceConfig::default()),
            secrets: vec![ManifestSecret {
                name: "FOO".into(),
                item: None,
                field: None,
                destination_names: Vec::new(),
            }],
            destinations: vec![ManifestDestination::EnvFile(EnvFileDestinationConfig {
                path: env_path,
            })],
        };
        let manifest_path = dir.join("gh-secrets.json");
        manifest.save(&manifest_path).unwrap();
        manifest_path
    }

    #[test]
    fn end_to_end_env_file_round_trip() {
        let dir = TempDir::new().unwrap();
        let env_path = PathBuf::from(".env");
        let manifest_path = manifest_with_env_file(dir.path(), env_path);
        let source = StaticSource::new(vec![("FOO", "bar")]);
        let report = sync_manifest_with_source(&manifest_path, None, &source).unwrap();
        assert_eq!(report.destinations.len(), 1);
        assert_eq!(report.destinations[0].report.created, vec!["FOO"]);

        let env_path_abs = dir.path().join(".env");
        let content = std::fs::read_to_string(&env_path_abs).unwrap();
        assert_eq!(content, "FOO=\"bar\"\n");

        // Re-running with same source value should be a no-op.
        let report2 = sync_manifest_with_source(&manifest_path, None, &source).unwrap();
        assert!(report2.is_noop());
        assert_eq!(report2.destinations[0].report.unchanged, vec!["FOO"]);
    }

    #[test]
    fn one_source_value_fans_out_to_multiple_destination_names() {
        let dir = TempDir::new().unwrap();
        // Source identity is `npm-token`; it must land under two GitHub-style
        // names in the destination.
        let manifest = RepoManifest {
            source: ManifestSource::Bitwarden(BitwardenSourceConfig::default()),
            secrets: vec![ManifestSecret {
                name: "npm-token".into(),
                item: None,
                field: None,
                destination_names: vec!["NPM_TOKEN".into(), "NODE_AUTH_TOKEN".into()],
            }],
            destinations: vec![ManifestDestination::EnvFile(EnvFileDestinationConfig {
                path: PathBuf::from(".env"),
            })],
        };
        let manifest_path = dir.path().join("gh-secrets.json");
        manifest.save(&manifest_path).unwrap();

        let source = StaticSource::new(vec![("npm-token", "s3cr3t")]);
        let report = sync_manifest_with_source(&manifest_path, None, &source).unwrap();
        // Both destination names are created from the single source value.
        assert_eq!(
            report.destinations[0].report.created,
            vec!["NPM_TOKEN", "NODE_AUTH_TOKEN"]
        );

        let content = std::fs::read_to_string(dir.path().join(".env")).unwrap();
        assert!(content.contains("NPM_TOKEN=\"s3cr3t\""));
        assert!(content.contains("NODE_AUTH_TOKEN=\"s3cr3t\""));
        // The source-side identity is never written as a destination key.
        assert!(!content.contains("npm-token="));

        // Re-running is a no-op for both fanned-out names.
        let report2 = sync_manifest_with_source(&manifest_path, None, &source).unwrap();
        assert!(report2.is_noop());
        assert_eq!(
            report2.destinations[0].report.unchanged,
            vec!["NPM_TOKEN", "NODE_AUTH_TOKEN"]
        );
    }

    #[test]
    fn source_change_propagates_to_env_file() {
        let dir = TempDir::new().unwrap();
        let manifest_path = manifest_with_env_file(dir.path(), PathBuf::from(".env"));
        let source = StaticSource::new(vec![("FOO", "v1")]);
        let _ = sync_manifest_with_source(&manifest_path, None, &source).unwrap();
        let source2 = StaticSource::new(vec![("FOO", "v2")]);
        let report = sync_manifest_with_source(&manifest_path, None, &source2).unwrap();
        assert_eq!(report.destinations[0].report.updated, vec!["FOO"]);
        let content = std::fs::read_to_string(dir.path().join(".env")).unwrap();
        assert!(content.contains("FOO=\"v2\""));
    }
}
