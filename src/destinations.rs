//! Sync destinations: where the orchestrator pushes values to.
//!
//! Three concrete impls today:
//! - `GitHubDestination` — wraps the GitHub Actions secrets client, uses
//!   `GH_TOKEN` / `GITHUB_TOKEN` for auth. Write-only: GitHub never returns a
//!   secret's value, so there is no matching source.
//! - `EnvFileDestination` — writes a dotenv-style file, preserving unrelated
//!   lines and (on Unix) tightening file mode to 0600.
//! - `LocalStoreDestination` — writes into the encrypted vault's secret map
//!   (the write half of the `local` store).

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::github::GitHubClient;
use crate::manifest::{EnvFileDestinationConfig, GithubDestinationConfig};

/// One value the orchestrator wants pushed, with everything the destination
/// needs to decide whether the push is a no-op. Borrows from the engine's
/// fetched entries (and the sync state for `last_pushed_hash`) so building a
/// per-destination request never copies a secret value.
#[derive(Debug, Clone, Copy)]
pub struct DestinationEntry<'a> {
    pub name: &'a str,
    pub value: &'a str,
    pub current_hash: &'a str,
    pub last_pushed_hash: Option<&'a str>,
}

#[derive(Debug, Clone, Default)]
pub struct DestinationRequest<'a> {
    pub entries: Vec<DestinationEntry<'a>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DestinationReport {
    pub created: Vec<String>,
    pub updated: Vec<String>,
    pub unchanged: Vec<String>,
}

impl DestinationReport {
    pub fn changed(&self) -> bool {
        !self.created.is_empty() || !self.updated.is_empty()
    }
}

/// What a destination knows how to do: identify itself (so the state file can
/// key on it) and apply a batch of updates.
pub trait Destination {
    fn key(&self) -> String;
    fn apply(&mut self, request: DestinationRequest<'_>) -> Result<DestinationReport>;
}

// ---- GitHub ----

pub const GITHUB_TOKEN_ENVS: &[&str] = &["GH_TOKEN", "GITHUB_TOKEN"];

pub struct GitHubDestination {
    repository: String,
    client: GitHubClient,
}

impl GitHubDestination {
    /// Build from a manifest github destination and an already-resolved token.
    /// Token resolution (env → stored config) lives in `crate::credentials` so
    /// every credential follows the same precedence; this just consumes the
    /// result.
    pub fn from_config(config: &GithubDestinationConfig, token: &str) -> Result<Self> {
        let client = GitHubClient::new(token).context("building GitHub client")?;
        Ok(Self {
            repository: config.repository.clone(),
            client,
        })
    }

    pub fn with_client(repository: String, client: GitHubClient) -> Self {
        Self { repository, client }
    }
}

/// First non-empty GitHub token among the canonical env vars (`GH_TOKEN`,
/// `GITHUB_TOKEN`), including any value loaded from `.env`/`.env.local`. Returns
/// `None` so the caller can fall back to stored config before erroring.
pub fn github_token_from_env() -> Option<String> {
    for name in GITHUB_TOKEN_ENVS {
        if let Ok(v) = env::var(name) {
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

impl Destination for GitHubDestination {
    fn key(&self) -> String {
        format!("github:{}", self.repository)
    }

    fn apply(&mut self, request: DestinationRequest<'_>) -> Result<DestinationReport> {
        let mut report = DestinationReport::default();
        // Lazily fetch the public key — only if we have at least one push.
        let mut public_key: Option<crate::github::RepoPublicKey> = None;
        for entry in &request.entries {
            if entry.last_pushed_hash == Some(entry.current_hash) {
                report.unchanged.push(entry.name.to_string());
                continue;
            }
            if public_key.is_none() {
                public_key = Some(
                    self.client
                        .get_public_key(&self.repository)
                        .with_context(|| format!("fetching public key for {}", self.repository))?,
                );
            }
            let key = public_key.as_ref().expect("public key set above");
            let ct = crate::github::seal(&key.key, entry.value)
                .with_context(|| format!("encrypting '{}' for {}", entry.name, self.repository))?;
            let created = self
                .client
                .put_secret(&self.repository, entry.name, &ct, &key.key_id)
                .with_context(|| format!("uploading '{}' to {}", entry.name, self.repository))?;
            if created {
                report.created.push(entry.name.to_string());
            } else {
                report.updated.push(entry.name.to_string());
            }
        }
        Ok(report)
    }
}

// ---- Env file ----

pub struct EnvFileDestination {
    pub path: PathBuf,
    /// State-file key derived from the manifest's (possibly relative) path so
    /// it stays stable across invocations regardless of the absolute resolved
    /// location of `path`.
    key: String,
}

impl EnvFileDestination {
    pub fn from_config(config: &EnvFileDestinationConfig, base_dir: &std::path::Path) -> Self {
        let path = if config.path.is_absolute() {
            config.path.clone()
        } else {
            base_dir.join(&config.path)
        };
        let key = format!("env_file:{}", config.path.display());
        Self { path, key }
    }
}

impl Destination for EnvFileDestination {
    fn key(&self) -> String {
        self.key.clone()
    }

    fn apply(&mut self, request: DestinationRequest<'_>) -> Result<DestinationReport> {
        let existing = if self.path.exists() {
            fs::read_to_string(&self.path)
                .with_context(|| format!("reading {}", self.path.display()))?
        } else {
            String::new()
        };

        let mut lines: Vec<String> = if existing.is_empty() {
            Vec::new()
        } else {
            existing.split('\n').map(String::from).collect()
        };
        // If the file ended with a trailing newline, `split` produced a trailing
        // empty element. Drop it so we don't double up newlines on rewrite.
        if existing.ends_with('\n') {
            lines.pop();
        }

        let mut key_to_index: BTreeMap<String, usize> = BTreeMap::new();
        for (i, line) in lines.iter().enumerate() {
            if let Some(k) = parse_env_key(line) {
                key_to_index.insert(k, i);
            }
        }

        let mut report = DestinationReport::default();
        let mut changed = false;

        for entry in &request.entries {
            let new_line = format_env_line(entry.name, entry.value);
            let already_present = key_to_index.contains_key(entry.name);
            // We only treat this as truly unchanged when the file already has
            // the key AND the state says we last pushed this exact hash. Either
            // missing means the user (or another tool) edited the file out from
            // under us; rewrite.
            let state_matches = entry.last_pushed_hash == Some(entry.current_hash);
            if already_present && state_matches {
                // Make sure the line is exactly what we'd write today — if not,
                // overwrite (handles a user-edited line).
                let idx = key_to_index[entry.name];
                if lines[idx] == new_line {
                    report.unchanged.push(entry.name.to_string());
                    continue;
                }
                lines[idx] = new_line;
                report.updated.push(entry.name.to_string());
                changed = true;
                continue;
            }

            match key_to_index.get(entry.name).copied() {
                Some(idx) => {
                    if lines[idx] != new_line {
                        lines[idx] = new_line;
                        changed = true;
                    }
                    report.updated.push(entry.name.to_string());
                }
                None => {
                    lines.push(new_line);
                    // Update the index in case a later entry has the same name
                    // (shouldn't happen, but be safe).
                    key_to_index.insert(entry.name.to_string(), lines.len() - 1);
                    report.created.push(entry.name.to_string());
                    changed = true;
                }
            }
        }

        if changed {
            let mut body = lines.join("\n");
            body.push('\n');
            write_env_file(&self.path, &body)?;
        }
        Ok(report)
    }
}

// ---- Local store ----

/// The global encrypted local store as a destination. Because the store is
/// readable, "unchanged" is decided by comparing the actual stored value, not
/// the state-file hash — same philosophy as the env-file destination's
/// content check.
pub struct LocalStoreDestination {
    pub vault_path: PathBuf,
}

impl LocalStoreDestination {
    pub fn new(vault_path: &std::path::Path) -> Self {
        Self {
            vault_path: vault_path.to_path_buf(),
        }
    }
}

impl Destination for LocalStoreDestination {
    fn key(&self) -> String {
        "local".to_string()
    }

    fn apply(&mut self, request: DestinationRequest<'_>) -> Result<DestinationReport> {
        let mut data = crate::vault::load(&self.vault_path)?;
        let mut report = DestinationReport::default();
        let mut changed = false;
        for entry in &request.entries {
            match data.secrets.get(entry.name) {
                Some(existing) if existing == entry.value => {
                    report.unchanged.push(entry.name.to_string());
                }
                Some(_) => {
                    data.secrets
                        .insert(entry.name.to_string(), entry.value.to_string());
                    report.updated.push(entry.name.to_string());
                    changed = true;
                }
                None => {
                    data.secrets
                        .insert(entry.name.to_string(), entry.value.to_string());
                    report.created.push(entry.name.to_string());
                    changed = true;
                }
            }
        }
        if changed {
            crate::vault::save(&self.vault_path, &data)?;
        }
        Ok(report)
    }
}

fn write_env_file(path: &std::path::Path, body: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    let tmp = match path.extension().and_then(|s| s.to_str()) {
        Some(ext) => path.with_extension(format!("{ext}.tmp")),
        None => path.with_extension("tmp"),
    };
    fs::write(&tmp, body).with_context(|| format!("writing {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(&tmp, perms)
            .with_context(|| format!("setting 0600 perms on {} before rename", tmp.display()))?;
    }
    fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Pull the `KEY` out of `KEY=value` (optionally prefixed by `export `).
/// Whitespace-tolerant on the left. Returns `None` for blank lines, comments,
/// or anything that doesn't look like an env assignment.
pub fn parse_env_key(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let rest = trimmed
        .strip_prefix("export ")
        .map(str::trim_start)
        .unwrap_or(trimmed);
    let eq = rest.find('=')?;
    let key = rest[..eq].trim_end();
    if key.is_empty() {
        return None;
    }
    if !key
        .chars()
        .next()
        .map(|c| c.is_ascii_alphabetic() || c == '_')
        .unwrap_or(false)
    {
        return None;
    }
    if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some(key.to_string())
}

/// Render `KEY=value` for an env file using double-quoted form with escapes
/// chosen so dotenv-style parsers (and Bash via `set -a; source file`) read
/// back the same value, while preventing variable expansion of `$`.
pub fn format_env_line(name: &str, value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for c in value.chars() {
        match c {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '$' => quoted.push_str("\\$"),
            '`' => quoted.push_str("\\`"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            _ => quoted.push(c),
        }
    }
    quoted.push('"');
    format!("{name}={quoted}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parse_env_key_handles_basic_cases() {
        assert_eq!(parse_env_key("FOO=bar"), Some("FOO".into()));
        assert_eq!(parse_env_key("  FOO=bar"), Some("FOO".into()));
        assert_eq!(parse_env_key("export FOO=bar"), Some("FOO".into()));
        assert_eq!(parse_env_key("FOO = bar"), Some("FOO".into()));
        assert_eq!(parse_env_key("# FOO=bar"), None);
        assert_eq!(parse_env_key(""), None);
        assert_eq!(parse_env_key("1FOO=bar"), None);
        assert_eq!(parse_env_key("foo-bar=1"), None);
    }

    #[test]
    fn format_env_line_escapes_safely() {
        assert_eq!(format_env_line("K", "v"), r#"K="v""#);
        assert_eq!(format_env_line("K", r#"a"b\c$d`e"#), r#"K="a\"b\\c\$d\`e""#);
        assert_eq!(format_env_line("K", "a\nb\tc"), r#"K="a\nb\tc""#);
    }

    /// Owned fixture mirroring what the engine holds; `request` lends it out
    /// as the borrowed entries `apply` takes, like `sync_with_source` does.
    struct OwnedEntry {
        name: String,
        value: String,
        current_hash: String,
        last_pushed_hash: Option<String>,
    }

    fn entry(name: &str, value: &str, last: Option<&str>) -> OwnedEntry {
        OwnedEntry {
            name: name.to_string(),
            value: value.to_string(),
            current_hash: crate::manifest::value_hash(name, value),
            last_pushed_hash: last.map(String::from),
        }
    }

    fn request(entries: &[OwnedEntry]) -> DestinationRequest<'_> {
        DestinationRequest {
            entries: entries
                .iter()
                .map(|e| DestinationEntry {
                    name: &e.name,
                    value: &e.value,
                    current_hash: &e.current_hash,
                    last_pushed_hash: e.last_pushed_hash.as_deref(),
                })
                .collect(),
        }
    }

    #[test]
    fn env_file_creates_new_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("out.env");
        let mut dest = EnvFileDestination {
            path: path.clone(),
            key: format!("env_file:{}", path.display()),
        };
        let report = dest.apply(request(&[entry("FOO", "bar", None)])).unwrap();
        assert_eq!(report.created, vec!["FOO"]);
        assert!(report.updated.is_empty());
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "FOO=\"bar\"\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn env_file_preserves_foreign_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("out.env");
        fs::write(&path, "# comment\nOTHER=keepme\nFOO=old\nTRAILING=keep\n").unwrap();
        let mut dest = EnvFileDestination {
            path: path.clone(),
            key: format!("env_file:{}", path.display()),
        };
        let report = dest
            .apply(request(&[
                entry("FOO", "new", None),
                entry("BAR", "added", None),
            ]))
            .unwrap();
        assert_eq!(report.updated, vec!["FOO"]);
        assert_eq!(report.created, vec!["BAR"]);
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(
            content,
            "# comment\nOTHER=keepme\nFOO=\"new\"\nTRAILING=keep\nBAR=\"added\"\n"
        );
    }

    #[test]
    fn env_file_skips_when_state_matches_and_file_already_has_value() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("out.env");
        let h = crate::manifest::value_hash("FOO", "bar");
        fs::write(&path, "FOO=\"bar\"\n").unwrap();
        let mut dest = EnvFileDestination {
            path: path.clone(),
            key: format!("env_file:{}", path.display()),
        };
        let report = dest
            .apply(request(&[entry("FOO", "bar", Some(&h))]))
            .unwrap();
        assert_eq!(report.unchanged, vec!["FOO"]);
        assert!(!report.changed());
    }

    #[test]
    fn local_store_destination_upserts_into_vault() {
        std::env::set_var(crate::vault::PASSPHRASE_ENV, "test-pass");
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("vault.json");
        let mut dest = LocalStoreDestination::new(&path);

        let report = dest.apply(request(&[entry("FOO", "v1", None)])).unwrap();
        assert_eq!(report.created, vec!["FOO"]);

        // Same value again: unchanged, decided by the stored value itself.
        let report = dest.apply(request(&[entry("FOO", "v1", None)])).unwrap();
        assert_eq!(report.unchanged, vec!["FOO"]);

        // New value: updated, and readable back through the vault.
        let report = dest.apply(request(&[entry("FOO", "v2", None)])).unwrap();
        assert_eq!(report.updated, vec!["FOO"]);
        let data = crate::vault::load(&path).unwrap();
        assert_eq!(data.secrets.get("FOO").map(String::as_str), Some("v2"));
        std::env::remove_var(crate::vault::PASSPHRASE_ENV);
    }

    #[test]
    fn env_file_rewrites_when_file_was_edited_even_if_state_matches() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("out.env");
        let h = crate::manifest::value_hash("FOO", "bar");
        // User edited the file to something else.
        fs::write(&path, "FOO=tampered\n").unwrap();
        let mut dest = EnvFileDestination {
            path: path.clone(),
            key: format!("env_file:{}", path.display()),
        };
        let report = dest
            .apply(request(&[entry("FOO", "bar", Some(&h))]))
            .unwrap();
        // The fact that state hash matches doesn't matter — file content
        // differs from canonical form, so we rewrite.
        assert_eq!(report.updated, vec!["FOO"]);
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("FOO=\"bar\""));
    }
}
