use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};

use crate::config::{load_active_profile, ProfileConfig};
use crate::github::GitHubClient;
use crate::paths::Paths;
use crate::secrets::Upsert;
use crate::sync::{self, SyncReport};

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
