use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};

use crate::credentials::StoredCredentials;
use crate::destinations::{DestinationReport, GITHUB_TOKEN_ENVS};
use crate::engine::{self, ConfigOrigin, ConfigSelector, Pipeline, PipelineOverrides};
use crate::envfile;
use crate::manifest::{
    BitwardenSourceConfig, EnvFileDestinationConfig, EnvFileSourceConfig, GithubDestinationConfig,
    Manifest, ManifestDestination, ManifestSecret, ManifestSource, DEFAULT_MANIFEST_FILE,
};
use crate::paths::Paths;
use crate::sources::{
    SourceItem, BW_CLIENTID_ENVS, BW_CLIENTSECRET_ENVS, BW_PASSWORD_ENVS, BW_SESSION_ENVS,
};
use crate::vault;

#[derive(Debug, Parser)]
#[command(
    name = "gh-secrets",
    version,
    about = "Sync secrets from a source (Bitwarden, env file, local store) to destinations \
             (GitHub Actions, env file, local store)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Write a starter config: `./gh-secrets.json` for this project, or the
    /// global config with `--global`.
    Init {
        /// Write the global config (under the config root) instead of a
        /// project-local one.
        #[arg(long)]
        global: bool,
        /// Where to write the project-local config. Defaults to
        /// `./gh-secrets.json`. Incompatible with `--global`.
        #[arg(long, short, conflicts_with = "global")]
        path: Option<PathBuf>,
    },
    /// List the secrets the resolved config declares (names and their source
    /// mapping), without contacting the source or printing any value.
    List {
        #[command(flatten)]
        config: ConfigOpts,
    },
    /// Pull every managed secret from the source and push to each destination
    /// that doesn't already hold the current value. Uses `./gh-secrets.json`
    /// (falling back to the global config) unless `--from`/`--to`/`--secret`
    /// override it.
    Sync {
        #[command(flatten)]
        config: ConfigOpts,
        #[command(flatten)]
        pipeline: PipelineOpts,
    },
    /// Read-only dry run: report which secrets a `sync` would push, per
    /// destination. Contacts only the source; needs no GitHub token and
    /// writes nothing.
    Check {
        #[command(flatten)]
        config: ConfigOpts,
        #[command(flatten)]
        pipeline: PipelineOpts,
    },
    /// Inspect the source the config pulls from (e.g. the Bitwarden vault):
    /// discover which item names are available to reference. Never prints a
    /// value.
    Source {
        #[command(subcommand)]
        command: SourceCmd,
    },
    /// Manage the global encrypted local store — a read/write store usable as
    /// `--from local` / `--to local`. Values are encrypted at rest in the
    /// vault; names are never printed alongside values.
    Store {
        #[command(subcommand)]
        command: StoreCmd,
    },
    /// Store credentials (GitHub token, Bitwarden login), encrypted at rest,
    /// as the lowest-priority fallback. Resolution order is: shell env > .env
    /// > .env.local > this stored config.
    Auth {
        #[command(subcommand)]
        command: AuthCmd,
    },
}

/// Which config to operate on, shared by `list`/`sync`/`check`/`source list`.
#[derive(Debug, Args)]
struct ConfigOpts {
    /// Path to a config file. Defaults to `./gh-secrets.json`, falling back to
    /// the global config.
    #[arg(long, short)]
    config: Option<PathBuf>,
    /// Use the global config (under the config root) even when the working
    /// directory has its own `gh-secrets.json`.
    #[arg(long, conflicts_with = "config")]
    global: bool,
    /// Path to the sync-state file. Defaults to `.gh-secrets-state.json` next
    /// to the resolved config.
    #[arg(long)]
    state: Option<PathBuf>,
}

impl ConfigOpts {
    fn selector(&self) -> ConfigSelector {
        ConfigSelector {
            config: self.config.clone(),
            global: self.global,
            state: self.state.clone(),
        }
    }
}

/// Arg-level pipeline overrides: each provided section replaces the
/// corresponding section of the resolved config, so a config file is never
/// required.
#[derive(Debug, Args)]
struct PipelineOpts {
    /// Source to pull from: `bitwarden`, `env:<path>`, or `local`.
    /// (`github:...` is write-only and cannot be a source.)
    #[arg(long)]
    from: Option<String>,
    /// Destination to push to (repeatable): `github:<owner>/<repo>`,
    /// `env:<path>`, or `local`.
    #[arg(long)]
    to: Vec<String>,
    /// Secret to manage (repeatable): `NAME`, `NAME=ITEM`, or
    /// `NAME=ITEM#FIELD` (e.g. `API_KEY=my-bw-item#fields.API_KEY`).
    #[arg(long = "secret")]
    secrets: Vec<String>,
    /// Limit this run to the named secret(s) among those declared
    /// (repeatable).
    #[arg(long)]
    only: Vec<String>,
    /// Bitwarden collection id to scope lookups to (with `--from bitwarden`).
    #[arg(long, requires = "from")]
    collection_id: Option<String>,
    /// Bitwarden organization id to scope lookups to (with `--from
    /// bitwarden`).
    #[arg(long, requires = "from")]
    organization_id: Option<String>,
    /// Default field to extract when a secret doesn't specify one (with
    /// `--from bitwarden`). Defaults to `password`.
    #[arg(long, requires = "from")]
    default_field: Option<String>,
}

impl PipelineOpts {
    fn overrides(&self) -> Result<PipelineOverrides> {
        let source = match &self.from {
            Some(spec) => Some(parse_source_spec(
                spec,
                self.collection_id.clone(),
                self.organization_id.clone(),
                self.default_field.clone(),
            )?),
            None => {
                if self.collection_id.is_some()
                    || self.organization_id.is_some()
                    || self.default_field.is_some()
                {
                    bail!("--collection-id/--organization-id/--default-field require `--from bitwarden`");
                }
                None
            }
        };
        let destinations = self
            .to
            .iter()
            .map(|s| parse_destination_spec(s))
            .collect::<Result<Vec<_>>>()?;
        let secrets = self
            .secrets
            .iter()
            .map(|s| parse_secret_spec(s))
            .collect::<Result<Vec<_>>>()?;
        Ok(PipelineOverrides {
            source,
            destinations,
            secrets,
            only: self.only.clone(),
        })
    }
}

#[derive(Debug, Subcommand)]
enum SourceCmd {
    /// List the items available in the source (e.g. every entry in the
    /// Bitwarden vault, scoped by the config's collection/organization).
    /// Prints each item's name and id — never a value.
    List {
        #[command(flatten)]
        config: ConfigOpts,
        /// Source to enumerate instead of the config's: `bitwarden`,
        /// `env:<path>`, or `local`.
        #[arg(long)]
        from: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum StoreCmd {
    /// Set a value in the local store. Reads the value from stdin when not
    /// given as an argument (preferred: keeps it out of shell history).
    Set { name: String, value: Option<String> },
    /// Remove a value from the local store.
    Remove { name: String },
    /// List the names in the local store (never values).
    List,
}

#[derive(Debug, Subcommand)]
enum AuthCmd {
    /// Store a GitHub token used by `github` destinations.
    Github {
        /// Personal access token with `repo` scope (or fine-grained
        /// `secrets:write`).
        token: String,
    },
    /// Store Bitwarden login material (personal API key + master password).
    /// Each flag is optional; only the ones you pass are updated.
    Bitwarden {
        /// Bitwarden personal API key client id (the `user.<uuid>` value).
        #[arg(long)]
        client_id: Option<String>,
        /// Bitwarden personal API key client secret.
        #[arg(long)]
        client_secret: Option<String>,
        /// Bitwarden master password (still required to unlock the vault even
        /// with an API key).
        #[arg(long)]
        master_password: Option<String>,
    },
    /// Show where each credential resolves from (env, .env, .env.local, or
    /// stored config) without ever printing its value.
    Status,
    /// Remove stored credentials. With no flag, clears everything.
    Clear {
        /// Only clear the stored GitHub token.
        #[arg(long)]
        github: bool,
        /// Only clear the stored Bitwarden login.
        #[arg(long)]
        bitwarden: bool,
    },
}

/// Entry point invoked from `main`.
pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::resolve()?;
    let cwd = PathBuf::from(".");

    match cli.command {
        Command::Init { global, path } => {
            let (target, starter) = if global {
                (paths.global_manifest_file(), Manifest::global_starter())
            } else {
                (
                    path.unwrap_or_else(|| PathBuf::from(DEFAULT_MANIFEST_FILE)),
                    Manifest::starter(),
                )
            };
            if target.exists() {
                bail!("{} already exists; refusing to overwrite", target.display());
            }
            starter.save(&target)?;
            println!("init: wrote starter to {}", target.display());
        }

        Command::List { config } => {
            // Reads only config metadata — no source contact, no credentials.
            let pipeline = engine::resolve_pipeline(
                &paths,
                &cwd,
                &config.selector(),
                PipelineOverrides::default(),
                false,
            )?;
            list_declared_secrets(&pipeline);
        }

        Command::Sync { config, pipeline } => {
            // Make `.env`/`.env.local` in the working directory available as
            // credentials before resolving the source/destinations.
            envfile::load_dotenv_cwd();
            let overrides = pipeline.overrides()?;
            let resolved =
                engine::resolve_pipeline(&paths, &cwd, &config.selector(), overrides, true)?;
            let report = engine::sync(&paths, &resolved)?;
            print_sync_report(&report);
        }

        Command::Check { config, pipeline } => {
            envfile::load_dotenv_cwd();
            let overrides = pipeline.overrides()?;
            let resolved =
                engine::resolve_pipeline(&paths, &cwd, &config.selector(), overrides, true)?;
            let report = engine::check(&paths, &resolved)?;
            print_check_report(&resolved, &report);
        }

        Command::Source { command } => match command {
            SourceCmd::List { config, from } => {
                // Same credential resolution as `sync`: load `.env`/`.env.local`
                // before unlocking the source.
                envfile::load_dotenv_cwd();
                let overrides = PipelineOverrides {
                    source: match &from {
                        Some(spec) => Some(parse_source_spec(spec, None, None, None)?),
                        None => None,
                    },
                    ..Default::default()
                };
                let resolved =
                    engine::resolve_pipeline(&paths, &cwd, &config.selector(), overrides, false)?;
                let items = engine::list_source_items(&paths, &resolved)?;
                print_source_items(&items);
            }
        },

        Command::Store { command } => {
            // The vault passphrase may live in `.env`/`.env.local`.
            envfile::load_dotenv_cwd();
            let vault_path = paths.vault_file();
            match command {
                StoreCmd::Set { name, value } => {
                    if name.is_empty() {
                        bail!("secret name cannot be empty");
                    }
                    let value = match value {
                        Some(v) => v,
                        None => read_value_from_stdin(&name)?,
                    };
                    let mut data = vault::load(&vault_path)?;
                    let existed = data.secrets.insert(name.clone(), value).is_some();
                    vault::save(&vault_path, &data)?;
                    println!(
                        "store: {} '{name}'",
                        if existed { "updated" } else { "set" }
                    );
                }
                StoreCmd::Remove { name } => {
                    let mut data = vault::load(&vault_path)?;
                    if data.secrets.remove(&name).is_none() {
                        bail!("store has no secret named '{name}'");
                    }
                    if data.is_empty() && vault_path.exists() {
                        vault::remove(&vault_path)?;
                    } else {
                        vault::save(&vault_path, &data)?;
                    }
                    println!("store: removed '{name}'");
                }
                StoreCmd::List => {
                    let data = vault::load(&vault_path)?;
                    if data.secrets.is_empty() {
                        println!("store: empty");
                    } else {
                        println!("store ({}):", data.secrets.len());
                        for name in data.secrets.keys() {
                            println!("  - {name}");
                        }
                    }
                }
            }
        }

        Command::Auth { command } => {
            // The vault passphrase (and `auth status` provenance) may come
            // from `.env`/`.env.local`.
            let origins = envfile::load_dotenv_cwd();
            let vault_path = paths.vault_file();
            match command {
                AuthCmd::Github { token } => {
                    if token.is_empty() {
                        bail!("token cannot be empty");
                    }
                    let mut data = vault::load(&vault_path)?;
                    data.credentials.github_token = Some(token);
                    vault::save(&vault_path, &data)?;
                    println!("auth: stored GitHub token");
                }
                AuthCmd::Bitwarden {
                    client_id,
                    client_secret,
                    master_password,
                } => {
                    if client_id.is_none() && client_secret.is_none() && master_password.is_none() {
                        bail!(
                            "provide at least one of --client-id, --client-secret, --master-password"
                        );
                    }
                    let mut data = vault::load(&vault_path)?;
                    let mut set = Vec::new();
                    if let Some(v) = client_id {
                        data.credentials.bitwarden.client_id = Some(v);
                        set.push("client id");
                    }
                    if let Some(v) = client_secret {
                        data.credentials.bitwarden.client_secret = Some(v);
                        set.push("client secret");
                    }
                    if let Some(v) = master_password {
                        data.credentials.bitwarden.master_password = Some(v);
                        set.push("master password");
                    }
                    vault::save(&vault_path, &data)?;
                    println!("auth: stored Bitwarden {}", set.join(", "));
                }
                AuthCmd::Status => {
                    let stored = vault::load(&vault_path)?.credentials;
                    print_auth_status(&origins, &stored);
                }
                AuthCmd::Clear { github, bitwarden } => {
                    let mut data = vault::load(&vault_path)?;
                    // No flag means clear everything.
                    let clear_all = !github && !bitwarden;
                    if github || clear_all {
                        data.credentials.github_token = None;
                    }
                    if bitwarden || clear_all {
                        data.credentials.bitwarden = Default::default();
                    }
                    if data.is_empty() {
                        vault::remove(&vault_path)?;
                    } else {
                        vault::save(&vault_path, &data)?;
                    }
                    println!("auth: cleared stored credentials");
                }
            }
        }
    }
    Ok(())
}

// ---- store-spec parsing ----

/// Parse a `--from` spec. GitHub is rejected here because the store is
/// write-only: there is no API to read a secret's value back.
fn parse_source_spec(
    spec: &str,
    collection_id: Option<String>,
    organization_id: Option<String>,
    default_field: Option<String>,
) -> Result<ManifestSource> {
    match split_spec(spec) {
        ("bitwarden", None) => Ok(ManifestSource::Bitwarden(BitwardenSourceConfig {
            collection_id,
            organization_id,
            default_field,
        })),
        ("local", None) => Ok(ManifestSource::Local),
        ("env" | "env_file", Some(path)) if !path.is_empty() => {
            Ok(ManifestSource::EnvFile(EnvFileSourceConfig {
                path: PathBuf::from(path),
            }))
        }
        ("github", _) => bail!(
            "github is write-only (GitHub never returns a secret's value), so it can only be a --to destination"
        ),
        _ => bail!(
            "invalid source '{spec}': expected `bitwarden`, `env:<path>`, or `local`"
        ),
    }
}

/// Parse a `--to` spec.
fn parse_destination_spec(spec: &str) -> Result<ManifestDestination> {
    match split_spec(spec) {
        ("github", Some(repo)) if repo.split('/').filter(|p| !p.is_empty()).count() == 2 => {
            Ok(ManifestDestination::Github(GithubDestinationConfig {
                repository: repo.to_string(),
            }))
        }
        ("github", _) => bail!("invalid destination '{spec}': expected `github:<owner>/<repo>`"),
        ("env" | "env_file", Some(path)) if !path.is_empty() => {
            Ok(ManifestDestination::EnvFile(EnvFileDestinationConfig {
                path: PathBuf::from(path),
            }))
        }
        ("local", None) => Ok(ManifestDestination::Local),
        ("bitwarden", _) => {
            bail!("bitwarden is not yet supported as a destination (it is read-only today)")
        }
        _ => bail!(
            "invalid destination '{spec}': expected `github:<owner>/<repo>`, `env:<path>`, or `local`"
        ),
    }
}

fn split_spec(spec: &str) -> (&str, Option<&str>) {
    match spec.split_once(':') {
        Some((kind, rest)) => (kind, Some(rest)),
        None => (spec, None),
    }
}

/// Parse a `--secret` spec: `NAME`, `NAME=ITEM`, or `NAME=ITEM#FIELD`.
fn parse_secret_spec(spec: &str) -> Result<ManifestSecret> {
    let (name, rest) = match spec.split_once('=') {
        Some((n, r)) => (n, Some(r)),
        None => (spec, None),
    };
    if name.is_empty() {
        bail!("invalid secret '{spec}': name is empty");
    }
    let (item, field) = match rest {
        None => (None, None),
        Some(r) => match r.rsplit_once('#') {
            Some((item, field)) if !field.is_empty() => (nonempty(item), Some(field.to_string())),
            _ => (nonempty(r), None),
        },
    };
    if rest.is_some() && item.is_none() {
        bail!("invalid secret '{spec}': item is empty");
    }
    Ok(ManifestSecret {
        name: name.to_string(),
        item,
        field,
    })
}

fn nonempty(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn read_value_from_stdin(name: &str) -> Result<String> {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        return rpassword::prompt_password(format!("value for '{name}': "))
            .context("reading value");
    }
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .context("reading value from stdin")?;
    let value = buf.trim_end_matches(['\n', '\r']).to_string();
    if value.is_empty() {
        bail!("no value provided on stdin");
    }
    Ok(value)
}

// ---- output ----

/// Print the secrets the resolved config declares and where each is sourced
/// from. Reads only mapping metadata — never a value.
fn list_declared_secrets(pipeline: &Pipeline) {
    let source_label = pipeline.source.label();
    let origin = match &pipeline.origin {
        ConfigOrigin::File(path) => format!("{}", path.display()),
        ConfigOrigin::Args => "arguments".to_string(),
    };
    if pipeline.secrets.is_empty() {
        println!("list: no secrets declared (config: {origin}, source: {source_label})");
        return;
    }
    println!(
        "secrets ({}, config: {origin}, source: {source_label}):",
        pipeline.secrets.len()
    );
    for s in &pipeline.secrets {
        match &pipeline.source {
            ManifestSource::Bitwarden(c) => {
                // Mirror `BitwardenSource::default_field`: unspecified means
                // `password`.
                let field = s
                    .field
                    .as_deref()
                    .or(c.default_field.as_deref())
                    .unwrap_or("password");
                println!(
                    "  - {}  (bitwarden item '{}', field '{field}')",
                    s.name,
                    s.source_item()
                );
            }
            ManifestSource::EnvFile(c) => {
                println!(
                    "  - {}  (env file '{}', key '{}')",
                    s.name,
                    c.path.display(),
                    s.source_item()
                );
            }
            ManifestSource::Local => {
                println!("  - {}  (local store key '{}')", s.name, s.source_item());
            }
        }
    }
}

/// Print the items available in a source (name + id), sorted by name for
/// stable output. Identity metadata only — never a value.
fn print_source_items(items: &[SourceItem]) {
    if items.is_empty() {
        println!("source: no items available");
        return;
    }
    let mut sorted: Vec<&SourceItem> = items.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    println!("source items ({}):", sorted.len());
    for item in sorted {
        println!("  - {}  ({})", item.name, item.id);
    }
}

/// Report where each credential resolves from, without ever printing a value
/// (honoring the "never print a secret" invariant — tokens and the master
/// password are secrets too).
fn print_auth_status(origins: &BTreeMap<String, &'static str>, stored: &StoredCredentials) {
    println!("auth status (priority: shell env > .env > .env.local > stored config):");
    let rows: [(&str, &[&str], Option<&str>); 5] = [
        (
            "GitHub token",
            GITHUB_TOKEN_ENVS,
            stored.github_token.as_deref(),
        ),
        (
            "Bitwarden client id",
            BW_CLIENTID_ENVS,
            stored.bitwarden.client_id.as_deref(),
        ),
        (
            "Bitwarden client secret",
            BW_CLIENTSECRET_ENVS,
            stored.bitwarden.client_secret.as_deref(),
        ),
        (
            "Bitwarden master password",
            BW_PASSWORD_ENVS,
            stored.bitwarden.master_password.as_deref(),
        ),
        // The unlock session is read from the environment only, never stored.
        ("Bitwarden session", BW_SESSION_ENVS, None),
    ];
    for (label, names, stored_value) in rows {
        println!(
            "  {label}: {}",
            describe_source(names, origins, stored_value)
        );
    }
}

fn describe_source(
    names: &[&str],
    origins: &BTreeMap<String, &'static str>,
    stored: Option<&str>,
) -> String {
    for name in names {
        match std::env::var_os(name) {
            Some(v) if !v.is_empty() => {
                return match origins.get(*name) {
                    Some(file) => format!("set (from {file})"),
                    None => "set (from your shell environment)".to_string(),
                };
            }
            _ => {}
        }
    }
    if stored.is_some_and(|s| !s.is_empty()) {
        "set (from stored config)".to_string()
    } else {
        "not set".to_string()
    }
}

fn print_sync_report(report: &engine::SyncReport) {
    if report.is_noop() {
        println!("sync: nothing to do");
        return;
    }
    for outcome in &report.destinations {
        print_destination_report(&outcome.destination_key, &outcome.report);
    }
}

fn print_destination_report(key: &str, report: &DestinationReport) {
    for name in &report.created {
        println!("{key}: created '{name}'");
    }
    for name in &report.updated {
        println!("{key}: updated '{name}'");
    }
    println!(
        "{key}: {} created, {} updated, {} unchanged",
        report.created.len(),
        report.updated.len(),
        report.unchanged.len()
    );
}

fn print_check_report(pipeline: &Pipeline, report: &engine::CheckReport) {
    let origin = match &pipeline.origin {
        ConfigOrigin::File(path) => format!("{}", path.display()),
        ConfigOrigin::Args => "arguments".to_string(),
    };
    println!("check (config: {origin}):");
    let mut total_stale = 0usize;
    for dest in &report.destinations {
        if dest.stale.is_empty() {
            println!(
                "  {}: up to date ({} secret(s))",
                dest.destination_key,
                dest.up_to_date.len()
            );
        } else {
            total_stale += dest.stale.len();
            println!(
                "  {}: {} to push ({}), {} up to date",
                dest.destination_key,
                dest.stale.len(),
                dest.stale.join(", "),
                dest.up_to_date.len()
            );
        }
    }
    if total_stale == 0 {
        println!("check: everything is up to date");
    } else {
        println!("check: {total_stale} push(es) pending — run `gh-secrets sync`");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_spec_parses_name_item_field() {
        let s = parse_secret_spec("FOO").unwrap();
        assert_eq!((s.name.as_str(), s.item, s.field), ("FOO", None, None));

        let s = parse_secret_spec("FOO=my item").unwrap();
        assert_eq!(s.item.as_deref(), Some("my item"));
        assert_eq!(s.field, None);

        let s = parse_secret_spec("FOO=my item#fields.API_KEY").unwrap();
        assert_eq!(s.item.as_deref(), Some("my item"));
        assert_eq!(s.field.as_deref(), Some("fields.API_KEY"));

        assert!(parse_secret_spec("=item").is_err());
        assert!(parse_secret_spec("FOO=").is_err());
    }

    #[test]
    fn source_spec_enforces_capabilities() {
        assert!(matches!(
            parse_source_spec("bitwarden", None, None, None).unwrap(),
            ManifestSource::Bitwarden(_)
        ));
        assert!(matches!(
            parse_source_spec("local", None, None, None).unwrap(),
            ManifestSource::Local
        ));
        match parse_source_spec("env:.env.master", None, None, None).unwrap() {
            ManifestSource::EnvFile(c) => assert_eq!(c.path, PathBuf::from(".env.master")),
            other => panic!("unexpected: {other:?}"),
        }
        // GitHub is write-only.
        let err = parse_source_spec("github:o/r", None, None, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("write-only"), "got: {err}");
        assert!(parse_source_spec("nope", None, None, None).is_err());
    }

    #[test]
    fn destination_spec_enforces_capabilities() {
        assert!(matches!(
            parse_destination_spec("github:o/r").unwrap(),
            ManifestDestination::Github(_)
        ));
        assert!(matches!(
            parse_destination_spec("local").unwrap(),
            ManifestDestination::Local
        ));
        assert!(matches!(
            parse_destination_spec("env:.env").unwrap(),
            ManifestDestination::EnvFile(_)
        ));
        // Bitwarden writes aren't implemented yet.
        let err = parse_destination_spec("bitwarden").unwrap_err().to_string();
        assert!(err.contains("not yet supported"), "got: {err}");
        // A github destination needs owner/repo.
        assert!(parse_destination_spec("github:just-a-name").is_err());
        assert!(parse_destination_spec("github:").is_err());
    }
}
