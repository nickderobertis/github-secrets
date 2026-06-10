use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};

use crate::config::{load_active_profile, ProfileConfig};
use crate::credentials::StoredCredentials;
use crate::destinations::{DestinationReport, GITHUB_TOKEN_ENVS};
use crate::envfile;
use crate::github::GitHubClient;
use crate::manifest::{RepoManifest, DEFAULT_MANIFEST_FILE};
use crate::paths::Paths;
use crate::secrets::Upsert;
use crate::sources::{BW_CLIENTID_ENVS, BW_CLIENTSECRET_ENVS, BW_PASSWORD_ENVS, BW_SESSION_ENVS};
use crate::sync::{self, SyncReport};
use crate::sync_manifest::{self, ManifestSyncReport};

#[derive(Debug, Parser)]
#[command(
    name = "gh-secrets",
    version,
    about = "Manage GitHub Actions secrets in bulk"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Set the GitHub personal access token used for the active profile.
    Token {
        /// Personal access token (or fine-grained token) with `secrets:write`.
        token: String,
    },
    /// Manage profiles (independent stores of secrets and auth).
    Profile {
        #[command(subcommand)]
        command: ProfileCmd,
    },
    /// Manage which repositories the active profile syncs to.
    Repo {
        #[command(subcommand)]
        command: RepoCmd,
    },
    /// Manage secret values stored in the active profile.
    Secrets {
        #[command(subcommand)]
        command: SecretCmd,
    },
    /// Manage sync records (the "last pushed at" log).
    Record {
        #[command(subcommand)]
        command: RecordCmd,
    },
    /// Show which repositories aren't in the profile and which secrets are stale.
    Check,
    /// Work with a repo-local `gh-secrets.json` manifest: pull from an external
    /// source (Bitwarden) and push to one or more destinations (GitHub, env
    /// file). Independent of the profile-based commands above.
    Manifest {
        #[command(subcommand)]
        command: ManifestCmd,
    },
    /// Store credentials for the manifest flow (GitHub token, Bitwarden login)
    /// as the lowest-priority fallback. Resolution order is: shell env > .env >
    /// .env.local > this stored config.
    Auth {
        #[command(subcommand)]
        command: AuthCmd,
    },
}

#[derive(Debug, Subcommand)]
enum AuthCmd {
    /// Store a GitHub token used by manifest `github` destinations.
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

#[derive(Debug, Subcommand)]
enum ManifestCmd {
    /// Write a starter `gh-secrets.json` next to which `gh-secrets manifest
    /// sync` can be invoked.
    Init {
        /// Where to write the manifest. Defaults to `./gh-secrets.json`.
        #[arg(long, short)]
        path: Option<PathBuf>,
    },
    /// Pull every managed secret from the manifest's source and push to each
    /// destination that doesn't already hold the current value.
    Sync {
        /// Path to the manifest file. Defaults to `./gh-secrets.json`.
        #[arg(long, short)]
        config: Option<PathBuf>,
        /// Path to the sync-state file. Defaults to `.gh-secrets-state.json`
        /// next to the manifest.
        #[arg(long)]
        state: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum ProfileCmd {
    /// Create a new profile.
    Create { name: String },
    /// Delete a profile (cannot be undone).
    Delete { name: String },
    /// Set the active profile.
    Set { name: String },
}

#[derive(Debug, Subcommand)]
enum RepoCmd {
    /// Add a repository to the included list.
    Add(RepoArg),
    /// Remove a repository from the included list.
    Remove(RepoArg),
    /// Add a repository to the excluded list.
    AddExclude(RepoArg),
    /// Remove a repository from the excluded list.
    RemoveExclude(RepoArg),
    /// Discover repositories on GitHub and add them to the included list.
    Bootstrap,
}

#[derive(Debug, Args)]
struct RepoArg {
    /// Full name including the owner, e.g. `nickderobertis/github-secrets`.
    name: String,
}

#[derive(Debug, Subcommand)]
enum SecretCmd {
    /// Add (or update) a secret globally, or scoped to a single repository.
    Add {
        name: String,
        value: String,
        /// Optional repository for a per-repo override.
        repository: Option<String>,
    },
    /// Remove a secret globally, or for a single repository.
    Remove {
        name: String,
        repository: Option<String>,
    },
    /// Push changed secrets to GitHub.
    Sync {
        /// Optional secret name; if omitted, syncs every defined secret.
        name: Option<String>,
        /// Optional repository; if omitted, syncs to every included repo.
        repository: Option<String>,
        /// Print extra detail about skipped/up-to-date items.
        #[arg(long, short)]
        verbose: bool,
    },
}

#[derive(Debug, Subcommand)]
enum RecordCmd {
    /// Mark every secret as already synced (use on first-time adoption).
    Fill,
    /// Wipe all sync records (forces re-sync of everything). Cannot be undone.
    Reset,
}

/// Entry point invoked from `main`.
pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::resolve()?;
    let (mut app, mut profile) = load_active_profile(&paths)?;
    let mut app_dirty = false;
    let mut profile_dirty = false;

    match cli.command {
        Command::Token { token } => {
            profile.github_token = token;
            profile_dirty = true;
            println!("token: set for profile '{}'", app.current_profile);
        }

        Command::Profile { command } => match command {
            ProfileCmd::Create { name } => {
                ensure_profile_name(&name)?;
                if app.has_profile(&name) {
                    anyhow::bail!("profile '{name}' already exists");
                }
                app.add_profile(&name);
                // Touch a fresh profile file so the user can inspect it.
                ProfileConfig::default().save(&paths.profile_file(&name))?;
                app_dirty = true;
                println!("profile '{name}': created");
            }
            ProfileCmd::Delete { name } => {
                if !app.has_profile(&name) {
                    anyhow::bail!("profile '{name}' does not exist");
                }
                if app.current_profile == name {
                    anyhow::bail!(
                        "profile '{name}' is currently active; switch with `gh-secrets profile set <other>` first"
                    );
                }
                app.remove_profile(&name);
                let path = paths.profile_file(&name);
                if path.exists() {
                    std::fs::remove_file(&path)
                        .with_context(|| format!("removing {}", path.display()))?;
                }
                app_dirty = true;
                println!("profile '{name}': deleted");
            }
            ProfileCmd::Set { name } => {
                if !app.has_profile(&name) {
                    anyhow::bail!("profile '{name}' does not exist");
                }
                profile = ProfileConfig::load_or_default(&paths.profile_file(&name))?;
                println!("profile '{name}': active");
                app.current_profile = name;
                app_dirty = true;
            }
        },

        Command::Repo { command } => match command {
            RepoCmd::Add(RepoArg { name }) => {
                profile.add_include(&name)?;
                profile_dirty = true;
                println!("repo '{name}': included");
            }
            RepoCmd::Remove(RepoArg { name }) => {
                profile.remove_include(&name)?;
                profile_dirty = true;
                println!("repo '{name}': removed from included");
            }
            RepoCmd::AddExclude(RepoArg { name }) => {
                profile.add_exclude(&name)?;
                profile_dirty = true;
                println!("repo '{name}': excluded");
            }
            RepoCmd::RemoveExclude(RepoArg { name }) => {
                profile.remove_exclude(&name)?;
                profile_dirty = true;
                println!("repo '{name}': removed from excluded");
            }
            RepoCmd::Bootstrap => {
                let client = GitHubClient::new(&profile.github_token)?;
                let discovered = client.list_user_repositories()?;
                let excluded: Vec<String> =
                    profile.exclude_repositories.clone().unwrap_or_default();
                let mut added = 0usize;
                for repo in discovered {
                    if excluded.iter().any(|r| r == &repo) {
                        continue;
                    }
                    if profile.add_include(&repo).is_ok() {
                        added += 1;
                        println!("repo '{repo}': included");
                    }
                }
                profile_dirty = true;
                println!("bootstrap: {added} repo(s) added");
            }
        },

        Command::Secrets { command } => match command {
            SecretCmd::Add {
                name,
                value,
                repository,
            } => {
                let outcome = match repository.as_deref() {
                    Some(repo) => profile.repository_secrets.upsert(repo, &name, &value),
                    None => profile.global_secrets.upsert(&name, &value),
                };
                profile_dirty = true;
                let verb = match outcome {
                    Upsert::Created => "created",
                    Upsert::Updated => "updated",
                };
                println!(
                    "secret '{name}' ({}): {verb}",
                    scope_label(repository.as_deref())
                );
            }
            SecretCmd::Remove { name, repository } => {
                let removed = match repository.as_deref() {
                    Some(repo) => profile.repository_secrets.remove(repo, &name),
                    None => profile.global_secrets.remove(&name),
                };
                if !removed {
                    anyhow::bail!(
                        "secret '{name}' ({}) was not defined",
                        scope_label(repository.as_deref())
                    );
                }
                profile_dirty = true;
                println!(
                    "secret '{name}' ({}): removed",
                    scope_label(repository.as_deref())
                );
            }
            SecretCmd::Sync {
                name,
                repository,
                verbose,
            } => {
                let report = sync::sync(
                    &mut profile,
                    repository.as_deref(),
                    name.as_deref(),
                    verbose,
                )?;
                profile_dirty = true;
                print_sync_report(&report);
            }
        },

        Command::Record { command } => match command {
            RecordCmd::Fill => {
                let repos = profile.include_repositories.clone().unwrap_or_default();
                sync::record_fill(&mut profile, &repos);
                profile_dirty = true;
                println!("record fill: marked all secrets as synced");
            }
            RecordCmd::Reset => {
                profile.sync_records.clear();
                profile_dirty = true;
                println!("record reset: all sync records cleared");
            }
        },

        Command::Check => {
            check(&profile)?;
        }

        Command::Manifest { command } => match command {
            ManifestCmd::Init { path } => {
                let target = path.unwrap_or_else(|| PathBuf::from(DEFAULT_MANIFEST_FILE));
                if target.exists() {
                    anyhow::bail!("{} already exists; refusing to overwrite", target.display());
                }
                RepoManifest::starter().save(&target)?;
                println!("manifest: wrote starter to {}", target.display());
            }
            ManifestCmd::Sync { config, state } => {
                // Make `.env`/`.env.local` in the working directory available as
                // credentials before resolving the source/destinations.
                envfile::load_dotenv_cwd();
                let manifest_path = config.unwrap_or_else(|| PathBuf::from(DEFAULT_MANIFEST_FILE));
                let report = sync_manifest::sync_manifest(&manifest_path, state.as_deref())?;
                print_manifest_report(&report);
            }
        },

        Command::Auth { command } => {
            let creds_path = paths.credentials_file();
            let mut stored = StoredCredentials::load(&creds_path)?;
            match command {
                AuthCmd::Github { token } => {
                    if token.is_empty() {
                        anyhow::bail!("token cannot be empty");
                    }
                    stored.github_token = Some(token);
                    stored.save(&creds_path)?;
                    println!("auth: stored GitHub token");
                }
                AuthCmd::Bitwarden {
                    client_id,
                    client_secret,
                    master_password,
                } => {
                    if client_id.is_none() && client_secret.is_none() && master_password.is_none() {
                        anyhow::bail!(
                            "provide at least one of --client-id, --client-secret, --master-password"
                        );
                    }
                    let mut set = Vec::new();
                    if let Some(v) = client_id {
                        stored.bitwarden.client_id = Some(v);
                        set.push("client id");
                    }
                    if let Some(v) = client_secret {
                        stored.bitwarden.client_secret = Some(v);
                        set.push("client secret");
                    }
                    if let Some(v) = master_password {
                        stored.bitwarden.master_password = Some(v);
                        set.push("master password");
                    }
                    stored.save(&creds_path)?;
                    println!("auth: stored Bitwarden {}", set.join(", "));
                }
                AuthCmd::Status => {
                    let origins = envfile::load_dotenv_cwd();
                    print_auth_status(&origins, &stored);
                }
                AuthCmd::Clear { github, bitwarden } => {
                    // No flag means clear everything.
                    let clear_all = !github && !bitwarden;
                    if github || clear_all {
                        stored.github_token = None;
                    }
                    if bitwarden || clear_all {
                        stored.bitwarden = Default::default();
                    }
                    if stored.is_empty() && creds_path.exists() {
                        std::fs::remove_file(&creds_path)
                            .with_context(|| format!("removing {}", creds_path.display()))?;
                    } else {
                        stored.save(&creds_path)?;
                    }
                    println!("auth: cleared stored credentials");
                }
            }
        }
    }

    if profile_dirty {
        profile.save(&paths.profile_file(&app.current_profile))?;
    }
    if app_dirty {
        app.save(&paths.app_file())?;
    }
    Ok(())
}

fn ensure_profile_name(name: &str) -> Result<()> {
    if name == "app" {
        anyhow::bail!("'app' is reserved and cannot be used as a profile name");
    }
    if name.is_empty() {
        anyhow::bail!("profile name cannot be empty");
    }
    Ok(())
}

fn scope_label(repo: Option<&str>) -> String {
    match repo {
        Some(r) => format!("repo '{r}'"),
        None => "global".to_string(),
    }
}

/// Report where each manifest credential resolves from, without ever printing
/// a value (honoring the "never print a secret" invariant — tokens and the
/// master password are secrets too).
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

fn print_manifest_report(report: &ManifestSyncReport) {
    if report.is_noop() {
        println!("manifest sync: nothing to do");
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

fn print_sync_report(report: &SyncReport) {
    if report.is_noop() {
        println!("sync: nothing to do");
        return;
    }
    for (repo, name) in &report.created {
        println!("sync: created '{name}' in {repo}");
    }
    for (repo, name) in &report.updated {
        println!("sync: updated '{name}' in {repo}");
    }
    println!(
        "sync: {} created, {} updated, {} skipped",
        report.created.len(),
        report.updated.len(),
        report.skipped.len()
    );
}

fn check(profile: &ProfileConfig) -> Result<()> {
    if profile.github_token.is_empty() {
        anyhow::bail!("GitHub token is not set; run `gh-secrets token <token>` first");
    }
    let client = GitHubClient::new(&profile.github_token)?;
    let known: std::collections::BTreeSet<&str> = profile
        .include_repositories
        .iter()
        .flatten()
        .map(String::as_str)
        .collect();
    let excluded: std::collections::BTreeSet<&str> = profile
        .exclude_repositories
        .iter()
        .flatten()
        .map(String::as_str)
        .collect();
    let discovered = client.list_user_repositories()?;
    let new_repos: Vec<String> = discovered
        .into_iter()
        .filter(|r| !known.contains(r.as_str()) && !excluded.contains(r.as_str()))
        .collect();

    let unsynced = unsynced_secrets(profile);

    if new_repos.is_empty() && unsynced.is_empty() {
        println!("check: everything is up to date");
        return Ok(());
    }
    if !new_repos.is_empty() {
        println!("check: {} new repository(ies):", new_repos.len());
        for r in &new_repos {
            println!("  - {r}");
        }
    }
    if !unsynced.is_empty() {
        println!("check: {} unsynced secret(s):", unsynced.len());
        for (repo, name) in &unsynced {
            println!("  - {repo}  {name}");
        }
    }
    Ok(())
}

fn unsynced_secrets(profile: &ProfileConfig) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let repos = profile.include_repositories.clone().unwrap_or_default();
    for repo in &repos {
        // For each (global + repo-scoped) secret, decide whether it's been
        // synced to *this* repo at or after its last update.
        let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for s in &profile.global_secrets.secrets {
            names.insert(s.name.clone());
        }
        if let Some(list) = profile.repository_secrets.by_repo.get(repo) {
            for s in list {
                names.insert(s.name.clone());
            }
        }
        let records = profile.sync_records.get(repo);
        for name in names {
            let secret = profile
                .repository_secrets
                .get(repo, &name)
                .cloned()
                .or_else(|| profile.global_secrets.get(&name).cloned());
            let Some(secret) = secret else { continue };
            let last = records
                .and_then(|rs| rs.iter().find(|r| r.secret_name == name))
                .map(|r| r.last_synced);
            let stale = match last {
                Some(ts) => ts < secret.updated,
                None => true,
            };
            if stale {
                out.push((repo.clone(), name));
            }
        }
    }
    out
}
