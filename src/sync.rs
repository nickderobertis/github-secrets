use std::collections::BTreeSet;

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chrono::{DateTime, Utc};

use crate::config::ProfileConfig;
use crate::github::GitHubClient;
use crate::secrets::{Secret, SyncRecord};

/// What changed during a sync run; used by the CLI to print a tidy summary.
#[derive(Debug, Default)]
pub struct SyncReport {
    pub created: Vec<(String, String)>,
    pub updated: Vec<(String, String)>,
    pub skipped: Vec<(String, String)>,
}

impl SyncReport {
    /// True when the sync run made no upstream changes — i.e. every relevant
    /// secret was already up to date or out of scope. `skipped` does not count
    /// as work.
    pub fn is_noop(&self) -> bool {
        self.created.is_empty() && self.updated.is_empty()
    }
}

/// Push every secret that has changed since its last successful sync. If a
/// `repository` is given, only sync to that one repo; if a `secret_name` is
/// given, only sync that one name.
pub fn sync(
    profile: &mut ProfileConfig,
    repository: Option<&str>,
    secret_name: Option<&str>,
    verbose: bool,
) -> Result<SyncReport> {
    if profile.github_token.is_empty() {
        bail!("GitHub token is not set; run `gh-secrets token <token>` first");
    }
    let client = GitHubClient::new(&profile.github_token)?;
    let repositories = resolve_repositories(profile, repository, &client)?;
    let names = resolve_names(profile, secret_name);

    let mut report = SyncReport::default();
    for name in &names {
        for repo in &repositories {
            sync_one(profile, &client, name, repo, verbose, &mut report)?;
        }
    }
    Ok(report)
}

fn resolve_repositories(
    profile: &ProfileConfig,
    requested: Option<&str>,
    client: &GitHubClient,
) -> Result<Vec<String>> {
    if let Some(r) = requested {
        return Ok(vec![r.to_string()]);
    }
    if let Some(included) = &profile.include_repositories {
        return Ok(included.clone());
    }
    // No included list: discover from GitHub, minus excluded.
    let mut repos = client.list_user_repositories()?;
    if let Some(excluded) = &profile.exclude_repositories {
        let excluded: BTreeSet<&str> = excluded.iter().map(String::as_str).collect();
        repos.retain(|r| !excluded.contains(r.as_str()));
    }
    Ok(repos)
}

fn resolve_names(profile: &ProfileConfig, requested: Option<&str>) -> Vec<String> {
    if let Some(n) = requested {
        return vec![n.to_string()];
    }
    let mut names: BTreeSet<String> = BTreeSet::new();
    for s in &profile.global_secrets.secrets {
        names.insert(s.name.clone());
    }
    for list in profile.repository_secrets.by_repo.values() {
        for s in list {
            names.insert(s.name.clone());
        }
    }
    names.into_iter().collect()
}

fn sync_one(
    profile: &mut ProfileConfig,
    client: &GitHubClient,
    name: &str,
    repo: &str,
    verbose: bool,
    report: &mut SyncReport,
) -> Result<()> {
    // Repo override wins over global; otherwise fall back to the global secret
    // if one exists with that name.
    let secret: Secret = if let Some(s) = profile.repository_secrets.get(repo, name) {
        s.clone()
    } else if let Some(s) = profile.global_secrets.get(name) {
        s.clone()
    } else {
        // Neither global nor repo-scoped: nothing to do for this combo.
        if verbose {
            println!("skip {name} for {repo}: not defined");
        }
        report.skipped.push((repo.to_string(), name.to_string()));
        return Ok(());
    };

    let last_synced = secret_last_synced(profile, repo, name);
    if let Some(ts) = last_synced {
        if ts >= secret.updated {
            if verbose {
                println!("skip {name} for {repo}: already synced at {ts}");
            }
            report.skipped.push((repo.to_string(), name.to_string()));
            return Ok(());
        }
    }

    let key = client
        .get_public_key(repo)
        .with_context(|| format!("fetching public key for {repo}"))?;
    let ciphertext =
        seal(&key.key, &secret.value).with_context(|| format!("encrypting {name} for {repo}"))?;
    let created = client
        .put_secret(repo, name, &ciphertext, &key.key_id)
        .with_context(|| format!("uploading {name} for {repo}"))?;
    record_sync(profile, repo, name, Utc::now());
    if created {
        report.created.push((repo.to_string(), name.to_string()));
    } else {
        report.updated.push((repo.to_string(), name.to_string()));
    }
    Ok(())
}

fn secret_last_synced(profile: &ProfileConfig, repo: &str, name: &str) -> Option<DateTime<Utc>> {
    profile
        .sync_records
        .get(repo)?
        .iter()
        .find(|r| r.secret_name == name)
        .map(|r| r.last_synced)
}

pub fn record_sync(profile: &mut ProfileConfig, repo: &str, name: &str, at: DateTime<Utc>) {
    let records = profile.sync_records.entry(repo.to_string()).or_default();
    for r in records.iter_mut() {
        if r.secret_name == name {
            r.last_synced = at;
            return;
        }
    }
    records.push(SyncRecord {
        secret_name: name.to_string(),
        last_synced: at,
    });
}

/// Encrypts `plaintext` for the given base64-encoded X25519 public key using
/// libsodium-compatible sealed-box semantics, and returns the ciphertext as
/// base64. Matches what GitHub expects for `actions/secrets`.
pub fn seal(public_key_b64: &str, plaintext: &str) -> Result<String> {
    let key_bytes = B64.decode(public_key_b64).context("decoding public key")?;
    let key_array: [u8; 32] = key_bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("public key has wrong length (expected 32 bytes)"))?;
    let pk = crypto_box::PublicKey::from(key_array);
    let mut rng = crypto_box::aead::OsRng;
    let ciphertext = pk
        .seal(&mut rng, plaintext.as_bytes())
        .map_err(|_| anyhow::anyhow!("sealing failed"))?;
    Ok(B64.encode(ciphertext))
}

/// Mark every (repo, secret) combination as if it had been synced now. Used by
/// `gh-secrets record fill` on initial setup when the secrets already exist
/// in GitHub.
pub fn record_fill(profile: &mut ProfileConfig, repos: &[String]) {
    let now = Utc::now();
    let mut names: BTreeSet<String> = BTreeSet::new();
    for s in &profile.global_secrets.secrets {
        names.insert(s.name.clone());
    }
    for list in profile.repository_secrets.by_repo.values() {
        for s in list {
            names.insert(s.name.clone());
        }
    }
    for repo in repos {
        for name in &names {
            record_sync(profile, repo, name, now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_with(global: &[(&str, &str)], repo: &[(&str, &str, &str)]) -> ProfileConfig {
        let mut p = ProfileConfig::default();
        for (n, v) in global {
            p.global_secrets.upsert(n, v);
        }
        for (r, n, v) in repo {
            p.repository_secrets.upsert(r, n, v);
        }
        p
    }

    #[test]
    fn resolve_names_unions_global_and_repo() {
        let p = profile_with(&[("A", "1")], &[("o/r", "B", "2")]);
        let names = resolve_names(&p, None);
        assert_eq!(names, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn resolve_names_filters_to_requested() {
        let p = profile_with(&[("A", "1"), ("B", "2")], &[]);
        assert_eq!(resolve_names(&p, Some("A")), vec!["A".to_string()]);
    }

    #[test]
    fn record_sync_then_skips() {
        let mut p = profile_with(&[("A", "1")], &[]);
        // No record yet.
        assert!(secret_last_synced(&p, "o/r", "A").is_none());
        record_sync(&mut p, "o/r", "A", Utc::now());
        assert!(secret_last_synced(&p, "o/r", "A").is_some());
    }

    #[test]
    fn seal_with_a_real_pubkey_roundtrips_lengthwise() {
        // 32-byte public key encoded in base64.
        let key = [7u8; 32];
        let b64 = B64.encode(key);
        let ct = seal(&b64, "hello").unwrap();
        let raw = B64.decode(ct).unwrap();
        // Sealed box overhead: ephemeral pubkey (32) + MAC (16) on top of msg.
        assert_eq!(raw.len(), 32 + 16 + "hello".len());
    }
}
