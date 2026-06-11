//! The sync engine: resolve a pipeline (source → secrets → destinations) from
//! config + CLI overrides, pull every managed secret from the source, and push
//! to each destination that doesn't already hold the current value.
//!
//! Config resolution order (first hit wins):
//! 1. an explicit `--config <path>`
//! 2. `--global` → the global config under the config root
//! 3. `./gh-secrets.json` in the working directory
//! 4. the global config, if it exists
//! 5. nothing — in which case the CLI overrides must fully specify the
//!    pipeline.
//!
//! `--from`/`--to`/`--secret` each *replace* the corresponding section of
//! whatever config was resolved, so any manifest is reproducible as plain
//! arguments and a manifest is never required.

use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

use crate::credentials::StoredCredentials;
use crate::destinations::{
    Destination, DestinationEntry, DestinationReport, DestinationRequest, EnvFileDestination,
    GitHubDestination, LocalStoreDestination,
};
use crate::manifest::{
    value_hash, Manifest, ManifestDestination, ManifestSecret, ManifestSource, SyncState,
    DEFAULT_MANIFEST_FILE, DEFAULT_STATE_FILE,
};
use crate::paths::Paths;
use crate::sources::{
    BitwardenSource, EnvFileSource, LocalStoreSource, SecretSource, SourceItem, StaticSource,
};
use crate::vault;

/// Test-only override: if set, points at a JSON file `{ "NAME": "value", ... }`
/// that is used as a static source in place of the configured one. Lets the
/// e2e suites drive the binary without contacting Bitwarden. Intentionally
/// undocumented in `--help`, mirroring `GH_SECRETS_API_BASE`.
pub const TEST_SOURCE_FILE_ENV: &str = "GH_SECRETS_TEST_SOURCE_FILE";

/// Which config (if any) the pipeline was resolved from. Carried for output
/// and for deriving the default state-file location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigOrigin {
    /// A config file (project-local, global, or `--config`).
    File(PathBuf),
    /// No config file; the pipeline came entirely from CLI arguments.
    Args,
}

/// CLI-level selection of which config to use.
#[derive(Debug, Default)]
pub struct ConfigSelector {
    pub config: Option<PathBuf>,
    pub global: bool,
    pub state: Option<PathBuf>,
}

/// CLI-level overrides applied on top of the resolved config. Each populated
/// section replaces the config's section wholesale.
#[derive(Debug, Default)]
pub struct PipelineOverrides {
    pub source: Option<ManifestSource>,
    pub destinations: Vec<ManifestDestination>,
    pub secrets: Vec<ManifestSecret>,
    /// Filter applied after resolution: only these names are synced/checked.
    pub only: Vec<String>,
}

impl PipelineOverrides {
    pub fn is_empty(&self) -> bool {
        self.source.is_none()
            && self.destinations.is_empty()
            && self.secrets.is_empty()
            && self.only.is_empty()
    }
}

/// A fully resolved pipeline, ready to sync or check.
#[derive(Debug)]
pub struct Pipeline {
    pub source: ManifestSource,
    pub secrets: Vec<ManifestSecret>,
    pub destinations: Vec<ManifestDestination>,
    /// Directory relative paths (env files) resolve against.
    pub base_dir: PathBuf,
    pub state_path: PathBuf,
    pub origin: ConfigOrigin,
}

/// Resolve the pipeline for this invocation. `cwd` is the directory the local
/// `gh-secrets.json` (and arg-driven relative paths) resolve against — passed
/// explicitly so tests never depend on the process working directory.
/// `require_destinations` is true for sync/check (which need somewhere to
/// push) and false for commands that only need the source or the declared
/// secrets.
pub fn resolve_pipeline(
    paths: &Paths,
    cwd: &Path,
    selector: &ConfigSelector,
    overrides: PipelineOverrides,
    require_destinations: bool,
) -> Result<Pipeline> {
    let origin = resolve_config_origin(paths, cwd, selector, &overrides)?;
    let loaded: Option<Manifest> = match &origin {
        ConfigOrigin::File(path) => Some(
            Manifest::load(path)
                .with_context(|| format!("loading config from {}", path.display()))?,
        ),
        ConfigOrigin::Args => None,
    };

    // Take the loaded manifest apart so unoverridden sections move into the
    // pipeline instead of being cloned.
    let (loaded_source, loaded_secrets, loaded_destinations) = match loaded {
        Some(m) => (Some(m.source), m.secrets, m.destinations),
        None => (None, Vec::new(), Vec::new()),
    };
    let source = overrides.source.or(loaded_source).ok_or_else(|| {
        anyhow!("no source configured: pass --from, or create a config with `gh-secrets init`")
    })?;
    let destinations = if overrides.destinations.is_empty() {
        loaded_destinations
    } else {
        overrides.destinations
    };
    if require_destinations && destinations.is_empty() {
        bail!("no destinations configured: pass --to, or declare destinations in the config");
    }
    let mut secrets = if overrides.secrets.is_empty() {
        loaded_secrets
    } else {
        overrides.secrets
    };
    if !overrides.only.is_empty() {
        for name in &overrides.only {
            if !secrets.iter().any(|s| &s.name == name) {
                bail!("--only {name}: no secret with that name is declared");
            }
        }
        secrets.retain(|s| overrides.only.contains(&s.name));
    }
    // The same coherence guard `Manifest::load` applies, re-run on the
    // *resolved* set so `--secret` overrides can't smuggle in a destination
    // name collision (e.g. two `--secret FOO=...` entries).
    crate::manifest::validate_unique_destination_names(&secrets)?;

    let base_dir = match &origin {
        ConfigOrigin::File(path) => parent_or(path, cwd),
        ConfigOrigin::Args => cwd.to_path_buf(),
    };
    let state_path = match &selector.state {
        Some(p) => p.clone(),
        None => base_dir.join(DEFAULT_STATE_FILE),
    };

    Ok(Pipeline {
        source,
        secrets,
        destinations,
        base_dir,
        state_path,
        origin,
    })
}

fn resolve_config_origin(
    paths: &Paths,
    cwd: &Path,
    selector: &ConfigSelector,
    overrides: &PipelineOverrides,
) -> Result<ConfigOrigin> {
    if let Some(path) = &selector.config {
        if !path.exists() {
            bail!("config {} does not exist", path.display());
        }
        return Ok(ConfigOrigin::File(path.clone()));
    }
    if selector.global {
        let global = paths.global_manifest_file();
        if global.exists() {
            return Ok(ConfigOrigin::File(global));
        }
        // --global with a fully arg-specified pipeline is fine; otherwise the
        // user asked for a config that isn't there.
        if overrides.source.is_some() {
            return Ok(ConfigOrigin::Args);
        }
        bail!(
            "no global config at {}: run `gh-secrets init --global` first",
            global.display()
        );
    }
    let local = cwd.join(DEFAULT_MANIFEST_FILE);
    if local.exists() {
        return Ok(ConfigOrigin::File(local));
    }
    let global = paths.global_manifest_file();
    if global.exists() && overrides.source.is_none() {
        return Ok(ConfigOrigin::File(global));
    }
    Ok(ConfigOrigin::Args)
}

fn parent_or(path: &Path, fallback: &Path) -> PathBuf {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(fallback)
        .to_path_buf()
}

// ---- credentials ----

/// Credentials resolved lazily: the environment first, with the encrypted
/// vault decrypted at most once and only when something actually needs a
/// stored value (so e.g. a CI run with `GH_TOKEN` set never asks for a
/// passphrase).
pub struct LazyCredentials {
    vault_path: PathBuf,
    stored: RefCell<Option<StoredCredentials>>,
}

impl LazyCredentials {
    pub fn new(paths: &Paths) -> Self {
        Self {
            vault_path: paths.vault_file(),
            stored: RefCell::new(None),
        }
    }

    fn stored(&self) -> Result<StoredCredentials> {
        if let Some(s) = self.stored.borrow().as_ref() {
            return Ok(s.clone());
        }
        let creds = vault::load(&self.vault_path)?.credentials;
        *self.stored.borrow_mut() = Some(creds.clone());
        Ok(creds)
    }

    pub fn github_token(&self) -> Result<Option<String>> {
        if let Some(t) = crate::destinations::github_token_from_env() {
            return Ok(Some(t));
        }
        Ok(self.stored()?.github_token.filter(|s| !s.is_empty()))
    }

    pub fn bitwarden(&self) -> Result<crate::sources::BitwardenCredentials> {
        let from_env = crate::sources::BitwardenCredentials::from_env();
        // A session short-circuits login/unlock entirely; full env credentials
        // need no stored fallback either. Only touch the vault when a field is
        // actually missing.
        let fully_satisfied = from_env.session.is_some()
            || (from_env.client_id.is_some()
                && from_env.client_secret.is_some()
                && from_env.password.is_some());
        if fully_satisfied {
            return Ok(from_env);
        }
        let stored = self.stored()?;
        Ok(from_env.or_stored(
            stored.bitwarden.client_id.as_deref(),
            stored.bitwarden.client_secret.as_deref(),
            stored.bitwarden.master_password.as_deref(),
        ))
    }
}

// ---- store construction ----

fn build_source(
    source: &ManifestSource,
    creds: &LazyCredentials,
    base_dir: &Path,
    paths: &Paths,
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
            creds.bitwarden()?,
        ))),
        ManifestSource::EnvFile(cfg) => Ok(Box::new(EnvFileSource {
            path: resolve_relative(&cfg.path, base_dir),
        })),
        ManifestSource::Local => Ok(Box::new(LocalStoreSource::new(&paths.vault_file()))),
    }
}

fn build_destination(
    dest: &ManifestDestination,
    creds: &LazyCredentials,
    base_dir: &Path,
    paths: &Paths,
) -> Result<Box<dyn Destination>> {
    match dest {
        ManifestDestination::Github(c) => {
            let token = creds.github_token()?.ok_or_else(|| {
                anyhow!(
                    "no GitHub token for destination github:{}: set GH_TOKEN/GITHUB_TOKEN (e.g. in .env) or run `gh-secrets auth github <token>`",
                    c.repository
                )
            })?;
            Ok(Box::new(GitHubDestination::from_config(c, &token)?))
        }
        ManifestDestination::EnvFile(c) => {
            Ok(Box::new(EnvFileDestination::from_config(c, base_dir)))
        }
        ManifestDestination::Local => Ok(Box::new(LocalStoreDestination::new(&paths.vault_file()))),
    }
}

fn resolve_relative(path: &Path, base_dir: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
}

// ---- sync ----

#[derive(Debug, Default)]
pub struct SyncReport {
    pub destinations: Vec<DestinationOutcome>,
}

#[derive(Debug)]
pub struct DestinationOutcome {
    pub destination_key: String,
    pub report: DestinationReport,
}

impl SyncReport {
    pub fn is_noop(&self) -> bool {
        self.destinations.iter().all(|d| !d.report.changed())
    }
}

/// Run the pipeline: fetch all managed secrets from the source, push to every
/// destination missing the current value, and persist per-destination hashes.
pub fn sync(paths: &Paths, pipeline: &Pipeline) -> Result<SyncReport> {
    let creds = LazyCredentials::new(paths);
    let source = build_source(&pipeline.source, &creds, &pipeline.base_dir, paths)?;
    sync_with_source(paths, pipeline, source.as_ref(), &creds)
}

/// Same as [`sync`] but with a caller-supplied source — used by unit tests.
pub fn sync_with_source(
    paths: &Paths,
    pipeline: &Pipeline,
    source: &dyn SecretSource,
    creds: &LazyCredentials,
) -> Result<SyncReport> {
    let mut state = SyncState::load_or_default(&pipeline.state_path)?;
    let entries = fetch_entries(pipeline, source, &mut state)?;
    let hash_by_name: HashMap<&str, &str> = entries
        .iter()
        .map(|e| (e.name.as_str(), e.current_hash.as_str()))
        .collect();

    let mut report = SyncReport::default();
    for dest_cfg in &pipeline.destinations {
        let mut destination = build_destination(dest_cfg, creds, &pipeline.base_dir, paths)?;
        let dest_key = destination.key();
        // Per-destination request: inject the last-pushed hash from state.
        let mut req = DestinationRequest::default();
        for entry in &entries {
            req.entries.push(DestinationEntry {
                name: entry.name.clone(),
                value: entry.value.clone(),
                current_hash: entry.current_hash.clone(),
                last_pushed_hash: state
                    .last_pushed_hash(&entry.name, &dest_key)
                    .map(String::from),
            });
        }
        let dest_report = destination
            .apply(req)
            .with_context(|| format!("applying to destination {dest_key}"))?;
        // Record pushes — and "unchanged" results whose state was missing
        // (e.g. first run against a file that already had the value) — so we
        // converge on the no-op next time.
        for name in dest_report
            .created
            .iter()
            .chain(dest_report.updated.iter())
            .chain(dest_report.unchanged.iter())
        {
            if let Some(hash) = hash_by_name.get(name.as_str()) {
                state.record_push(name, &dest_key, hash);
            }
        }
        report.destinations.push(DestinationOutcome {
            destination_key: dest_key,
            report: dest_report,
        });
    }
    state.save(&pipeline.state_path)?;
    Ok(report)
}

fn fetch_entries(
    pipeline: &Pipeline,
    source: &dyn SecretSource,
    state: &mut SyncState,
) -> Result<Vec<DestinationEntry>> {
    let fetched = source
        .fetch(&pipeline.secrets)
        .context("fetching from source")?;
    // The source hash is recorded once under the secret's (source-side)
    // `name`; the fanned-out entries hash against their own destination name.
    for f in &fetched {
        state.record_source(&f.name, &value_hash(&f.name, &f.value));
    }
    fan_out(&pipeline.secrets, &fetched)
}

/// Fan each managed secret out into one destination entry per destination
/// name. The source returns one value per secret, keyed by the secret's
/// (source-side) `name`; index it so the order the source returned them in
/// doesn't matter. Each entry hashes against its own destination name, so the
/// same value written under two names tracks independently in the state file.
fn fan_out(
    secrets: &[ManifestSecret],
    fetched: &[crate::sources::FetchedSecret],
) -> Result<Vec<DestinationEntry>> {
    let fetched_by_name: HashMap<&str, &str> = fetched
        .iter()
        .map(|f| (f.name.as_str(), f.value.as_str()))
        .collect();
    let mut entries = Vec::with_capacity(secrets.iter().map(|s| s.dest_names().len()).sum());
    for secret in secrets {
        let value = *fetched_by_name.get(secret.name.as_str()).ok_or_else(|| {
            anyhow!(
                "source returned no value for managed secret '{}'",
                secret.name
            )
        })?;
        for dest_name in secret.dest_names() {
            entries.push(DestinationEntry {
                name: dest_name.to_string(),
                value: value.to_string(),
                current_hash: value_hash(dest_name, value),
                last_pushed_hash: None, // filled in per-destination
            });
        }
    }
    Ok(entries)
}

// ---- check ----

#[derive(Debug, Default)]
pub struct CheckReport {
    pub destinations: Vec<DestinationCheck>,
}

#[derive(Debug)]
pub struct DestinationCheck {
    pub destination_key: String,
    /// Names whose current source value doesn't match the last recorded push.
    pub stale: Vec<String>,
    pub up_to_date: Vec<String>,
}

impl CheckReport {
    pub fn is_clean(&self) -> bool {
        self.destinations.iter().all(|d| d.stale.is_empty())
    }
}

/// Read-only dry run: fetch current source values and report, per destination,
/// which secrets a `sync` would push. Contacts only the source — destinations
/// are judged purely from the recorded state, so no GitHub token is needed and
/// nothing is written (not even the state file).
pub fn check(paths: &Paths, pipeline: &Pipeline) -> Result<CheckReport> {
    let creds = LazyCredentials::new(paths);
    let source = build_source(&pipeline.source, &creds, &pipeline.base_dir, paths)?;
    let state = SyncState::load_or_default(&pipeline.state_path)?;
    let fetched = source
        .fetch(&pipeline.secrets)
        .context("fetching from source")?;
    // Judge the same fanned-out (destination-name, destination) pairs a sync
    // would push, so fan-out mappings are checked under their real names.
    let entries = fan_out(&pipeline.secrets, &fetched)?;

    let mut report = CheckReport::default();
    for dest_cfg in &pipeline.destinations {
        let dest_key = dest_cfg.key();
        let mut check = DestinationCheck {
            destination_key: dest_key.clone(),
            stale: Vec::new(),
            up_to_date: Vec::new(),
        };
        for entry in &entries {
            if state.last_pushed_hash(&entry.name, &dest_key) == Some(entry.current_hash.as_str()) {
                check.up_to_date.push(entry.name.clone());
            } else {
                check.stale.push(entry.name.clone());
            }
        }
        report.destinations.push(check);
    }
    Ok(report)
}

// ---- source listing ----

/// Enumerate the items available in the pipeline's source (e.g. the Bitwarden
/// vault, scoped by the config's collection/organization), so a user can
/// discover which item names exist to reference. Identity metadata only —
/// never a secret value.
pub fn list_source_items(paths: &Paths, pipeline: &Pipeline) -> Result<Vec<SourceItem>> {
    let creds = LazyCredentials::new(paths);
    let source = build_source(&pipeline.source, &creds, &pipeline.base_dir, paths)?;
    source.list_available()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{BitwardenSourceConfig, EnvFileDestinationConfig, EnvFileSourceConfig};
    use tempfile::TempDir;

    fn paths_in(dir: &Path) -> Paths {
        std::env::set_var("GH_SECRETS_HOME", dir.join("home"));
        Paths::resolve().unwrap()
    }

    fn pipeline_env_to_env(dir: &Path) -> Pipeline {
        Pipeline {
            source: ManifestSource::EnvFile(EnvFileSourceConfig {
                path: PathBuf::from("source.env"),
            }),
            secrets: vec![ManifestSecret {
                name: "FOO".into(),
                item: None,
                field: None,
                destination_names: Vec::new(),
            }],
            destinations: vec![ManifestDestination::EnvFile(EnvFileDestinationConfig {
                path: PathBuf::from("out.env"),
            })],
            base_dir: dir.to_path_buf(),
            state_path: dir.join(DEFAULT_STATE_FILE),
            origin: ConfigOrigin::Args,
        }
    }

    #[test]
    fn env_to_env_round_trip_and_noop() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(dir.path());
        fs::write(dir.path().join("source.env"), "FOO=\"bar\"\n").unwrap();
        let pipeline = pipeline_env_to_env(dir.path());

        let report = sync(&paths, &pipeline).unwrap();
        assert_eq!(report.destinations[0].report.created, vec!["FOO"]);
        let out = fs::read_to_string(dir.path().join("out.env")).unwrap();
        assert_eq!(out, "FOO=\"bar\"\n");

        // Same source values: no-op.
        let report = sync(&paths, &pipeline).unwrap();
        assert!(report.is_noop());

        // Source change propagates.
        fs::write(dir.path().join("source.env"), "FOO=\"v2\"\n").unwrap();
        let report = sync(&paths, &pipeline).unwrap();
        assert_eq!(report.destinations[0].report.updated, vec!["FOO"]);
    }

    #[test]
    fn check_reports_stale_then_clean_without_writing() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(dir.path());
        fs::write(dir.path().join("source.env"), "FOO=bar\n").unwrap();
        let pipeline = pipeline_env_to_env(dir.path());

        let report = check(&paths, &pipeline).unwrap();
        assert!(!report.is_clean());
        assert_eq!(report.destinations[0].stale, vec!["FOO"]);
        // Check must not create the state file or the destination.
        assert!(!pipeline.state_path.exists());
        assert!(!dir.path().join("out.env").exists());

        sync(&paths, &pipeline).unwrap();
        let report = check(&paths, &pipeline).unwrap();
        assert!(report.is_clean());
        assert_eq!(report.destinations[0].up_to_date, vec!["FOO"]);
    }

    #[test]
    fn overrides_replace_config_sections() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(dir.path());
        // A config whose destination we override away.
        let manifest = Manifest {
            source: ManifestSource::Bitwarden(BitwardenSourceConfig::default()),
            secrets: vec![ManifestSecret {
                name: "FOO".into(),
                item: None,
                field: None,
                destination_names: Vec::new(),
            }],
            destinations: vec![ManifestDestination::Github(
                crate::manifest::GithubDestinationConfig {
                    repository: "o/r".into(),
                },
            )],
        };
        let config_path = dir.path().join("gh-secrets.json");
        manifest.save(&config_path).unwrap();

        let overrides = PipelineOverrides {
            destinations: vec![ManifestDestination::EnvFile(EnvFileDestinationConfig {
                path: PathBuf::from(".env"),
            })],
            ..Default::default()
        };
        let selector = ConfigSelector {
            config: Some(config_path),
            ..Default::default()
        };
        let pipeline = resolve_pipeline(&paths, dir.path(), &selector, overrides, true).unwrap();
        // Source and secrets come from the config; destination was replaced.
        assert_eq!(pipeline.source, manifest.source);
        assert_eq!(pipeline.secrets, manifest.secrets);
        assert_eq!(
            pipeline.destinations,
            vec![ManifestDestination::EnvFile(EnvFileDestinationConfig {
                path: PathBuf::from(".env"),
            })]
        );
        // State sits next to the config file by default.
        assert_eq!(pipeline.state_path, dir.path().join(DEFAULT_STATE_FILE));
    }

    #[test]
    fn only_filters_and_rejects_unknown_names() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(dir.path());
        let overrides = PipelineOverrides {
            source: Some(ManifestSource::Local),
            destinations: vec![ManifestDestination::Local],
            secrets: vec![
                ManifestSecret {
                    name: "A".into(),
                    item: None,
                    field: None,
                    destination_names: Vec::new(),
                },
                ManifestSecret {
                    name: "B".into(),
                    item: None,
                    field: None,
                    destination_names: Vec::new(),
                },
            ],
            only: vec!["A".into()],
        };
        let pipeline = resolve_pipeline(
            &paths,
            dir.path(),
            &ConfigSelector::default(),
            overrides,
            true,
        )
        .unwrap();
        assert_eq!(pipeline.secrets.len(), 1);
        assert_eq!(pipeline.secrets[0].name, "A");

        let overrides = PipelineOverrides {
            source: Some(ManifestSource::Local),
            destinations: vec![ManifestDestination::Local],
            secrets: vec![],
            only: vec!["MISSING".into()],
        };
        let err = resolve_pipeline(
            &paths,
            dir.path(),
            &ConfigSelector::default(),
            overrides,
            true,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("MISSING"), "got: {err}");
    }

    #[test]
    fn lazy_credentials_prefer_env_and_fall_back_to_vault() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(dir.path());
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
        std::env::set_var(vault::PASSPHRASE_ENV, "test-pass");
        let mut data = vault::VaultData::default();
        data.credentials.github_token = Some("stored-gh".into());
        data.credentials.bitwarden.client_id = Some("stored-id".into());
        data.credentials.bitwarden.client_secret = Some("stored-secret".into());
        data.credentials.bitwarden.master_password = Some("stored-pw".into());
        vault::save(&paths.vault_file(), &data).unwrap();

        // Stored config is the fallback when the env is empty.
        let creds = LazyCredentials::new(&paths);
        assert_eq!(creds.github_token().unwrap().as_deref(), Some("stored-gh"));
        let bw = creds.bitwarden().unwrap();
        assert_eq!(bw.client_id.as_deref(), Some("stored-id"));
        assert_eq!(bw.password.as_deref(), Some("stored-pw"));

        // The environment wins where it is set; gaps still fill from stored.
        std::env::set_var("GH_TOKEN", "env-gh");
        std::env::set_var("BW_PASSWORD", "env-pw");
        let creds = LazyCredentials::new(&paths);
        assert_eq!(creds.github_token().unwrap().as_deref(), Some("env-gh"));
        let bw = creds.bitwarden().unwrap();
        assert_eq!(bw.password.as_deref(), Some("env-pw"));
        assert_eq!(bw.client_id.as_deref(), Some("stored-id"));
        std::env::remove_var("GH_TOKEN");
        std::env::remove_var("BW_PASSWORD");
        std::env::remove_var(vault::PASSPHRASE_ENV);
    }

    #[test]
    fn lazy_credentials_never_touch_a_missing_vault() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(dir.path());
        // No vault file and no passphrase anywhere: env-only resolution must
        // still work (this is the CI path).
        std::env::remove_var(vault::PASSPHRASE_ENV);
        std::env::set_var("GH_TOKEN", "env-only");
        let creds = LazyCredentials::new(&paths);
        assert_eq!(creds.github_token().unwrap().as_deref(), Some("env-only"));
        std::env::remove_var("GH_TOKEN");
        let creds = LazyCredentials::new(&paths);
        assert_eq!(creds.github_token().unwrap(), None);
    }

    #[test]
    fn missing_source_is_a_guided_error() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(dir.path());
        let err = resolve_pipeline(
            &paths,
            dir.path(),
            &ConfigSelector::default(),
            PipelineOverrides::default(),
            true,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("--from"), "got: {err}");
        assert!(err.contains("gh-secrets init"), "got: {err}");
    }
}
