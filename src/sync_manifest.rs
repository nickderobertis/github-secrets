//! Manifest-driven sync orchestrator: pull all managed secrets from the
//! configured source, push to every destination that's missing the current
//! value, and persist per-destination hashes for next time.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::destinations::{
    Destination, DestinationEntry, DestinationReport, DestinationRequest, EnvFileDestination,
    GitHubDestination,
};
use crate::manifest::{
    value_hash, ManifestDestination, ManifestSource, RepoManifest, SyncState, DEFAULT_STATE_FILE,
};
use crate::sources::{BitwardenSource, SecretSource, StaticSource};

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
    let source = resolve_source(&manifest.source)?;
    let report = run(&manifest, &mut state, source.as_ref(), manifest_path)?;
    state.save(&state_path)?;
    Ok(report)
}

fn resolve_source(source: &ManifestSource) -> Result<Box<dyn SecretSource>> {
    if let Ok(path) = env::var(TEST_SOURCE_FILE_ENV) {
        let bytes = fs::read(&path).with_context(|| format!("reading test source file {path}"))?;
        let values: HashMap<String, String> = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing test source file {path} as JSON"))?;
        return Ok(Box::new(StaticSource { values }));
    }
    match source {
        ManifestSource::Bitwarden(cfg) => Ok(Box::new(BitwardenSource::new(cfg.clone()))),
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
    let report = run(&manifest, &mut state, source, manifest_path)?;
    state.save(&state_path)?;
    Ok(report)
}

fn run(
    manifest: &RepoManifest,
    state: &mut SyncState,
    source: &dyn SecretSource,
    manifest_path: &Path,
) -> Result<ManifestSyncReport> {
    let base_dir = manifest_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let fetched = source
        .fetch(&manifest.secrets)
        .context("fetching from source")?;

    // Compute current hashes once.
    let mut entries: Vec<DestinationEntry> = Vec::with_capacity(fetched.len());
    for f in &fetched {
        let h = value_hash(&f.name, &f.value);
        state.record_source(&f.name, &h);
        entries.push(DestinationEntry {
            name: f.name.clone(),
            value: f.value.clone(),
            current_hash: h,
            last_pushed_hash: None, // filled in per-destination below
        });
    }

    let mut report = ManifestSyncReport::default();
    for dest_cfg in &manifest.destinations {
        let mut destination: Box<dyn Destination> = match dest_cfg {
            ManifestDestination::Github(c) => Box::new(GitHubDestination::from_config(c)?),
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
