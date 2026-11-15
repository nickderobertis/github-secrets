//! Secret sources: where the manifest pulls values from.
//!
//! `SecretSource` is the single-method trait the orchestrator drives. The
//! first concrete implementation is `BitwardenSource`, which shells out to
//! the official `bw` CLI authenticated via the Bitwarden personal API key
//! (`BW_CLIENTID` + `BW_CLIENTSECRET` + `BW_PASSWORD`).

use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::manifest::{BitwardenSourceConfig, ManifestSecret, SourceRef};
use crate::vault;

/// A plaintext value retrieved from a source. Callers MUST treat this as
/// sensitive and must never log or persist `value` outside the documented
/// destinations.
#[derive(Debug, Clone)]
pub struct FetchedSecret {
    pub name: String,
    pub value: String,
    /// What this fetch resolved the secret's identifier to, for the engine to
    /// save in the sync state; `None` for a source that keeps no ref.
    pub source_ref: Option<SourceRef>,
}

/// Identity metadata for one item available in a source. Carries no secret
/// value — only what a user needs to discover an item and reference it from a
/// manifest (its display name and stable id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceItem {
    pub name: String,
    pub id: String,
}

/// Anything that can hand the orchestrator the current values for a list of
/// manifest secrets.
pub trait SecretSource {
    /// Fetch every secret's current value. `saved` holds the refs earlier
    /// fetches returned, keyed by secret `name`; a source that keeps refs may
    /// use one to skip re-resolving an identifier, and must itself decide
    /// whether it is still valid.
    fn fetch(
        &self,
        secrets: &[ManifestSecret],
        saved: &HashMap<String, SourceRef>,
    ) -> Result<Vec<FetchedSecret>>;

    /// Enumerate the items available in the source (e.g. every entry in the
    /// Bitwarden vault, optionally scoped by the source config). Returns
    /// identity metadata only — never a secret value — so callers can discover
    /// which item names exist to wire into a manifest.
    fn list_available(&self) -> Result<Vec<SourceItem>>;
}

/// In-memory source used by tests and (eventually) by a `manifest dry-run`
/// flow.
#[derive(Debug, Default, Clone)]
pub struct StaticSource {
    pub values: HashMap<String, String>,
}

impl StaticSource {
    pub fn new<I, K, V>(entries: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            values: entries
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }
}

impl SecretSource for StaticSource {
    fn fetch(
        &self,
        secrets: &[ManifestSecret],
        _saved: &HashMap<String, SourceRef>,
    ) -> Result<Vec<FetchedSecret>> {
        fetch_from_map(&self.values, secrets, "static source")
    }

    fn list_available(&self) -> Result<Vec<SourceItem>> {
        Ok(map_items(&self.values))
    }
}

/// Shared lookup for the map-backed sources (static, env file, local store):
/// `item` (defaulting to the secret's name) is the key in the map, and the
/// Bitwarden-style `field` selector has no meaning, so specifying one is a
/// config error rather than something to silently ignore.
fn fetch_from_map(
    values: &HashMap<String, String>,
    secrets: &[ManifestSecret],
    what: &str,
) -> Result<Vec<FetchedSecret>> {
    let mut out = Vec::with_capacity(secrets.len());
    for s in secrets {
        if let Some(field) = &s.field {
            bail!(
                "secret '{}' specifies field '{field}', but a {what} has no fields",
                s.name
            );
        }
        let key = s.source_item();
        let value = values
            .get(key)
            .ok_or_else(|| anyhow!("{what} has no value for '{key}'"))?;
        out.push(FetchedSecret {
            name: s.name.clone(),
            value: value.clone(),
            source_ref: None,
        });
    }
    Ok(out)
}

/// Identity listing for a map-backed source: the key doubles as name and id.
fn map_items(values: &HashMap<String, String>) -> Vec<SourceItem> {
    values
        .keys()
        .map(|k| SourceItem {
            name: k.clone(),
            id: k.clone(),
        })
        .collect()
}

/// A dotenv-style file used as a source: each managed secret's `item` is a key
/// in the file. The read-half of the same store the `env_file` destination
/// writes.
pub struct EnvFileSource {
    /// Already resolved against the manifest's directory by the caller.
    pub path: PathBuf,
}

impl EnvFileSource {
    fn read_values(&self) -> Result<HashMap<String, String>> {
        let content = std::fs::read_to_string(&self.path)
            .with_context(|| format!("reading env file source {}", self.path.display()))?;
        Ok(crate::envfile::parse_dotenv(&content).into_iter().collect())
    }
}

impl SecretSource for EnvFileSource {
    fn fetch(
        &self,
        secrets: &[ManifestSecret],
        _saved: &HashMap<String, SourceRef>,
    ) -> Result<Vec<FetchedSecret>> {
        let values = self.read_values()?;
        fetch_from_map(
            &values,
            secrets,
            &format!("env file ({})", self.path.display()),
        )
    }

    fn list_available(&self) -> Result<Vec<SourceItem>> {
        Ok(map_items(&self.read_values()?))
    }
}

/// The global encrypted local store used as a source. Decrypting the vault may
/// resolve a passphrase (env or prompt); see `crate::vault`.
pub struct LocalStoreSource {
    pub vault_path: PathBuf,
}

impl LocalStoreSource {
    pub fn new(vault_path: &Path) -> Self {
        Self {
            vault_path: vault_path.to_path_buf(),
        }
    }

    fn read_values(&self) -> Result<HashMap<String, String>> {
        let data = vault::load(&self.vault_path)?;
        Ok(data.secrets.into_iter().collect())
    }
}

impl SecretSource for LocalStoreSource {
    fn fetch(
        &self,
        secrets: &[ManifestSecret],
        _saved: &HashMap<String, SourceRef>,
    ) -> Result<Vec<FetchedSecret>> {
        let values = self.read_values()?;
        fetch_from_map(&values, secrets, "local store")
    }

    fn list_available(&self) -> Result<Vec<SourceItem>> {
        Ok(map_items(&self.read_values()?))
    }
}

// ---- Bitwarden ----

/// Canonical env var names passed through to the `bw` subprocess. These match
/// the Bitwarden CLI's own convention, so anything `bw` reads natively keeps
/// working.
pub const BW_CLIENTID_ENV: &str = "BW_CLIENTID";
pub const BW_CLIENTSECRET_ENV: &str = "BW_CLIENTSECRET";
pub const BW_PASSWORD_ENV: &str = "BW_PASSWORD";
pub const BW_SESSION_ENV: &str = "BW_SESSION";

/// Env var names we *read* credentials from, in priority order: the canonical
/// `BW_*` name first, then the `BITWARDEN_*` alias many users prefer in a
/// `.env`. The first non-empty match wins; whatever we read is handed to the
/// `bw` subprocess under the canonical name above.
pub const BW_CLIENTID_ENVS: &[&str] = &[BW_CLIENTID_ENV, "BITWARDEN_CLIENT_ID"];
pub const BW_CLIENTSECRET_ENVS: &[&str] = &[BW_CLIENTSECRET_ENV, "BITWARDEN_CLIENT_SECRET"];
pub const BW_PASSWORD_ENVS: &[&str] = &[
    BW_PASSWORD_ENV,
    "BITWARDEN_MASTER_PASSWORD",
    "BITWARDEN_PASSWORD",
];
pub const BW_SESSION_ENVS: &[&str] = &[BW_SESSION_ENV, "BITWARDEN_SESSION"];

/// First non-empty value among `names`. Treats an env var set to the empty
/// string as unset so we never hand `bw` a blank password (which would make it
/// fall back to an interactive prompt against a closed stdin).
pub fn env_first(names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|n| env::var(n).ok().filter(|v| !v.is_empty()))
}

/// Resolved Bitwarden login material handed to a `BitwardenSource`. Each field
/// is optional so the caller can fill it from the environment, fall back to
/// stored config, or leave it unset (and let `ensure_session` produce a precise
/// error). `session`, when present, short-circuits login/unlock entirely.
#[derive(Debug, Clone, Default)]
pub struct BitwardenCredentials {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub password: Option<String>,
    pub session: Option<String>,
}

impl BitwardenCredentials {
    /// Read every field from the process environment (which includes any
    /// `.env`/`.env.local` already loaded), honoring the canonical `BW_*` names
    /// and their `BITWARDEN_*` aliases.
    pub fn from_env() -> Self {
        Self {
            client_id: env_first(BW_CLIENTID_ENVS),
            client_secret: env_first(BW_CLIENTSECRET_ENVS),
            password: env_first(BW_PASSWORD_ENVS),
            session: env_first(BW_SESSION_ENVS),
        }
    }

    /// Fill any field still unset from stored config. `session` is deliberately
    /// never sourced from config — it's an ephemeral unlock token, not a
    /// durable credential.
    pub fn or_stored(
        mut self,
        client_id: Option<&str>,
        client_secret: Option<&str>,
        password: Option<&str>,
    ) -> Self {
        fn nonempty(v: Option<&str>) -> Option<String> {
            v.filter(|s| !s.is_empty()).map(String::from)
        }
        self.client_id = self.client_id.or_else(|| nonempty(client_id));
        self.client_secret = self.client_secret.or_else(|| nonempty(client_secret));
        self.password = self.password.or_else(|| nonempty(password));
        self
    }
}

/// Thin layer over the `bw` CLI. Exists so the field-extraction and error
/// paths can be unit-tested without the binary on `$PATH`.
pub trait BwCli {
    fn status(&self) -> Result<BwStatus>;
    fn login_apikey(&self, client_id: &str, client_secret: &str) -> Result<()>;
    fn unlock(&self, password: &str) -> Result<String>;
    fn sync(&self, session: &str) -> Result<()>;
    fn get_item(&self, session: &str, identifier: &str) -> Result<Value>;
    /// Enumerate items in the vault, optionally scoped to a collection and/or
    /// organization. Returns identity metadata only (`name`, `id`).
    fn list_items(
        &self,
        session: &str,
        collection_id: Option<&str>,
        organization_id: Option<&str>,
    ) -> Result<Vec<SourceItem>>;
}

/// Minimal projection of a `bw list items` element: every Bitwarden item has a
/// stable `id` and a display `name`; we ignore the rest (including any value
/// fields) so nothing sensitive is deserialized here.
#[derive(Debug, Deserialize)]
struct BwListItem {
    id: String,
    name: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BwStatus {
    Unauthenticated,
    Locked,
    Unlocked,
    #[serde(other)]
    Other,
}

/// The production `bw` runner. Each call spawns the binary fresh, so a
/// `bw` upgrade between calls doesn't strand a stale process.
pub struct RealBwCli;

impl RealBwCli {
    fn run(args: &[&str], env: &[(&str, &str)]) -> Result<Output> {
        let mut cmd = Command::new("bw");
        cmd.args(args);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.output()
            .with_context(|| format!("running `bw {}` (is the Bitwarden CLI installed?)", args[0]))
    }

    fn expect_success(args: &[&str], out: &Output) -> Result<()> {
        if out.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        // bw stderr never carries the secret value, so it's safe to surface.
        bail!(
            "`bw {}` failed with status {}: {}",
            args[0],
            out.status,
            stderr.trim()
        );
    }
}

impl BwCli for RealBwCli {
    fn status(&self) -> Result<BwStatus> {
        let args = ["status"];
        let out = Self::run(&args, &[])?;
        Self::expect_success(&args, &out)?;
        let parsed: Value = serde_json::from_slice(&out.stdout)
            .context("parsing `bw status` JSON (unexpected format)")?;
        match parsed.get("status").and_then(Value::as_str) {
            Some("unauthenticated") => Ok(BwStatus::Unauthenticated),
            Some("locked") => Ok(BwStatus::Locked),
            Some("unlocked") => Ok(BwStatus::Unlocked),
            _ => Ok(BwStatus::Other),
        }
    }

    fn login_apikey(&self, client_id: &str, client_secret: &str) -> Result<()> {
        let args = ["login", "--apikey"];
        let out = Self::run(
            &args,
            &[
                (BW_CLIENTID_ENV, client_id),
                (BW_CLIENTSECRET_ENV, client_secret),
            ],
        )?;
        Self::expect_success(&args, &out)
    }

    fn unlock(&self, password: &str) -> Result<String> {
        let args = ["unlock", "--raw", "--passwordenv", BW_PASSWORD_ENV];
        let out = Self::run(&args, &[(BW_PASSWORD_ENV, password)])?;
        Self::expect_success(&args, &out)?;
        let session = String::from_utf8(out.stdout)
            .context("`bw unlock` did not return UTF-8")?
            .trim()
            .to_string();
        if session.is_empty() {
            bail!("`bw unlock` returned an empty session token");
        }
        Ok(session)
    }

    fn sync(&self, session: &str) -> Result<()> {
        let args = ["sync"];
        let out = Self::run(&args, &[(BW_SESSION_ENV, session)])?;
        Self::expect_success(&args, &out)
    }

    fn get_item(&self, session: &str, identifier: &str) -> Result<Value> {
        let args = ["get", "item", identifier];
        let out = Self::run(&args, &[(BW_SESSION_ENV, session)])?;
        Self::expect_success(&args, &out)?;
        serde_json::from_slice(&out.stdout)
            .with_context(|| format!("parsing `bw get item {identifier}` JSON"))
    }

    fn list_items(
        &self,
        session: &str,
        collection_id: Option<&str>,
        organization_id: Option<&str>,
    ) -> Result<Vec<SourceItem>> {
        let mut args: Vec<&str> = vec!["list", "items"];
        if let Some(cid) = collection_id {
            args.extend_from_slice(&["--collectionid", cid]);
        }
        if let Some(oid) = organization_id {
            args.extend_from_slice(&["--organizationid", oid]);
        }
        let out = Self::run(&args, &[(BW_SESSION_ENV, session)])?;
        Self::expect_success(&args, &out)?;
        let items: Vec<BwListItem> =
            serde_json::from_slice(&out.stdout).context("parsing `bw list items` JSON")?;
        Ok(items
            .into_iter()
            .map(|i| SourceItem {
                name: i.name,
                id: i.id,
            })
            .collect())
    }
}

pub struct BitwardenSource<C: BwCli = RealBwCli> {
    config: BitwardenSourceConfig,
    cli: C,
    /// Resolved login material. A present `session` short-circuits login/unlock.
    credentials: BitwardenCredentials,
}

impl BitwardenSource<RealBwCli> {
    /// Build a source whose credentials come from the environment only. The
    /// manifest orchestrator instead uses [`with_credentials`] so it can layer
    /// stored config under the environment.
    pub fn new(config: BitwardenSourceConfig) -> Self {
        Self::with_credentials(config, BitwardenCredentials::from_env())
    }

    pub fn with_credentials(
        config: BitwardenSourceConfig,
        credentials: BitwardenCredentials,
    ) -> Self {
        Self {
            config,
            cli: RealBwCli,
            credentials,
        }
    }
}

impl<C: BwCli> BitwardenSource<C> {
    pub fn with_cli(
        config: BitwardenSourceConfig,
        cli: C,
        credentials: BitwardenCredentials,
    ) -> Self {
        Self {
            config,
            cli,
            credentials,
        }
    }

    fn ensure_session(&self) -> Result<String> {
        if let Some(s) = &self.credentials.session {
            return Ok(s.clone());
        }
        let status = self.cli.status().context("checking `bw status`")?;
        if status == BwStatus::Unauthenticated {
            let client_id = self.credentials.client_id.as_deref().ok_or_else(|| {
                anyhow!(
                    "no Bitwarden client id: set {BW_CLIENTID_ENV} (or BITWARDEN_CLIENT_ID, e.g. in .env) or run `gh-secrets auth bitwarden --client-id <id>`"
                )
            })?;
            let client_secret = self.credentials.client_secret.as_deref().ok_or_else(|| {
                anyhow!(
                    "no Bitwarden client secret: set {BW_CLIENTSECRET_ENV} (or BITWARDEN_CLIENT_SECRET, e.g. in .env) or run `gh-secrets auth bitwarden --client-secret <secret>`"
                )
            })?;
            self.cli
                .login_apikey(client_id, client_secret)
                .context("`bw login --apikey` failed")?;
        }
        let password = self.credentials.password.as_deref().ok_or_else(|| {
            anyhow!(
                "no Bitwarden master password: set {BW_PASSWORD_ENV} (or BITWARDEN_MASTER_PASSWORD, e.g. in .env) or run `gh-secrets auth bitwarden --master-password <pw>`"
            )
        })?;
        let session = self.cli.unlock(password).context("`bw unlock` failed")?;
        self.cli.sync(&session).context("`bw sync` failed")?;
        Ok(session)
    }

    fn default_field(&self) -> &str {
        self.config.default_field.as_deref().unwrap_or("password")
    }

    /// Resolve `identifier` to one item and fetch it by id, returning the item
    /// and that id. A saved ref for this identifier is tried first; when it no
    /// longer fetches the same item, the vault listing (shared across one
    /// `fetch` through `listing`) is searched for an exact id or name match.
    /// `bw get item` is only ever handed an id, never a name: given a name it
    /// does a case-insensitive substring search, which could return the wrong
    /// item or fail once a similarly named item exists.
    fn resolve(
        &self,
        session: &str,
        identifier: &str,
        saved: Option<&SourceRef>,
        listing: &mut Option<Vec<SourceItem>>,
    ) -> Result<(Value, String)> {
        if let Some(r) = saved.filter(|r| r.source == BITWARDEN_REF_SOURCE && r.item == identifier)
        {
            // Any failure means the ref is stale; the error text is never read.
            if let Ok(item) = self.cli.get_item(session, &r.id) {
                if saved_ref_still_matches(&item, r) {
                    return Ok((item, r.id.clone()));
                }
            }
        }
        let items = match listing {
            Some(items) => items,
            None => listing.insert(
                self.cli
                    .list_items(session, None, None)
                    .context("listing Bitwarden items to find an exact match")?,
            ),
        };
        let id = resolve_exact(items, identifier)?;
        let item = self.cli.get_item(session, &id)?;
        Ok((item, id))
    }
}

/// `SourceRef::source` for refs this source writes.
const BITWARDEN_REF_SOURCE: &str = "bitwarden";

/// Longest candidate list a resolution error prints before "and N more".
const MAX_LISTED_CANDIDATES: usize = 20;

/// Whether a saved ref still names the item it was resolved to. A ref whose id
/// equals its identifier was resolved by id, so the id alone must match; any
/// other was resolved by name, so the item must also still carry that exact
/// name (a rename, or a hand-edited id pointing elsewhere, makes it stale).
fn saved_ref_still_matches(item: &Value, saved: &SourceRef) -> bool {
    let field = |k: &str| item.get(k).and_then(Value::as_str);
    if field("id") != Some(saved.id.as_str()) {
        return false;
    }
    saved.id == saved.item || field("name") == Some(saved.item.as_str())
}

/// Pick the one listed item `identifier` names exactly: an item whose id equals
/// it wins outright, otherwise exactly one item must carry it as its exact
/// (case-sensitive) name. Errors list candidates by name and id only.
fn resolve_exact(items: &[SourceItem], identifier: &str) -> Result<String> {
    if items.iter().any(|i| i.id == identifier) {
        return Ok(identifier.to_string());
    }
    let exact: Vec<&SourceItem> = items.iter().filter(|i| i.name == identifier).collect();
    match exact.as_slice() {
        [only] => return Ok(only.id.clone()),
        [] => {}
        several => bail!(
            "{} items match '{identifier}' exactly:{}\nset `item` to the intended item's ID",
            several.len(),
            candidate_lines(several)
        ),
    }
    let needle = identifier.to_lowercase();
    let near: Vec<&SourceItem> = items
        .iter()
        .filter(|i| i.name.to_lowercase().contains(&needle))
        .collect();
    if near.is_empty() {
        bail!(
            "no item is named exactly '{identifier}', and no item matched it; run `gh-secrets source list` to see the available item names and IDs"
        );
    }
    bail!(
        "no item is named exactly '{identifier}'; candidates:{}\nset `item` to the exact name or the ID of the intended item",
        candidate_lines(&near)
    )
}

fn candidate_lines(items: &[&SourceItem]) -> String {
    let mut out: String = items
        .iter()
        .take(MAX_LISTED_CANDIDATES)
        .map(|i| format!("\n  {}  ({})", i.name, i.id))
        .collect();
    if items.len() > MAX_LISTED_CANDIDATES {
        out.push_str(&format!(
            "\n  and {} more",
            items.len() - MAX_LISTED_CANDIDATES
        ));
    }
    out
}

impl<C: BwCli> SecretSource for BitwardenSource<C> {
    fn fetch(
        &self,
        secrets: &[ManifestSecret],
        saved: &HashMap<String, SourceRef>,
    ) -> Result<Vec<FetchedSecret>> {
        if secrets.is_empty() {
            return Ok(Vec::new());
        }
        let session = self.ensure_session()?;
        let mut listing = None;
        let mut out = Vec::with_capacity(secrets.len());
        for s in secrets {
            let identifier = s.source_item();
            let (item, id) = self
                .resolve(&session, identifier, saved.get(&s.name), &mut listing)
                .with_context(|| format!("fetching '{}' from Bitwarden", s.name))?;
            let field = s.field.as_deref().unwrap_or_else(|| self.default_field());
            let value = extract_field(&item, field).with_context(|| {
                format!("extracting field '{field}' from Bitwarden item '{identifier}'")
            })?;
            out.push(FetchedSecret {
                name: s.name.clone(),
                value,
                source_ref: Some(SourceRef {
                    source: BITWARDEN_REF_SOURCE.to_string(),
                    item: identifier.to_string(),
                    id,
                }),
            });
        }
        Ok(out)
    }

    fn list_available(&self) -> Result<Vec<SourceItem>> {
        let session = self.ensure_session()?;
        self.cli.list_items(
            &session,
            self.config.collection_id.as_deref(),
            self.config.organization_id.as_deref(),
        )
    }
}

/// Pulls a field out of a Bitwarden item's JSON.
///
/// Supported field specs:
/// - `password` / `login.password`
/// - `username` / `login.username`
/// - `notes`
/// - `fields.<NAME>` for custom fields
pub fn extract_field(item: &Value, field: &str) -> Result<String> {
    match field {
        "password" | "login.password" => item
            .pointer("/login/password")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| anyhow!("item has no login.password")),
        "username" | "login.username" => item
            .pointer("/login/username")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| anyhow!("item has no login.username")),
        "notes" => item
            .get("notes")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| anyhow!("item has no notes")),
        other if other.starts_with("fields.") => {
            let target = &other["fields.".len()..];
            let arr = item
                .get("fields")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("item has no custom fields"))?;
            for f in arr {
                if f.get("name").and_then(Value::as_str) == Some(target) {
                    return f
                        .get("value")
                        .and_then(Value::as_str)
                        .map(String::from)
                        .ok_or_else(|| anyhow!("custom field '{target}' has no string value"));
                }
            }
            Err(anyhow!("no custom field named '{target}'"))
        }
        other => Err(anyhow!(
            "unsupported field selector '{other}' (supported: password, username, notes, fields.<NAME>)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;

    #[test]
    fn extract_password_field() {
        let item = json!({"login": {"password": "secret", "username": "alice"}});
        assert_eq!(extract_field(&item, "password").unwrap(), "secret");
        assert_eq!(extract_field(&item, "login.password").unwrap(), "secret");
        assert_eq!(extract_field(&item, "username").unwrap(), "alice");
    }

    #[test]
    fn extract_custom_field() {
        let item = json!({
            "fields": [
                {"name": "API_URL", "value": "https://x", "type": 0},
                {"name": "OTHER", "value": "y", "type": 1}
            ]
        });
        assert_eq!(extract_field(&item, "fields.API_URL").unwrap(), "https://x");
        assert!(extract_field(&item, "fields.MISSING").is_err());
    }

    #[test]
    fn extract_notes() {
        let item = json!({"notes": "abc"});
        assert_eq!(extract_field(&item, "notes").unwrap(), "abc");
    }

    #[test]
    fn extract_unsupported_field_is_an_error() {
        let item = json!({});
        let err = extract_field(&item, "totp").unwrap_err().to_string();
        assert!(
            err.contains("unsupported field selector 'totp'"),
            "got: {err}"
        );
        // The error must name the supported selectors so the user knows what
        // to try instead of just learning their guess was wrong.
        assert!(
            err.contains("password") && err.contains("fields.<NAME>"),
            "got: {err}"
        );
    }

    /// Mock CLI over an in-memory vault. Like the real `bw get item <id>`, it
    /// answers `get_item` by exact id only, so a test fails if the source ever
    /// hands it a name. Records every call so tests can assert the sequence.
    struct MockBw {
        status: BwStatus,
        vault: Vec<Value>,
        calls: RefCell<Vec<String>>,
    }

    impl MockBw {
        fn new(status: BwStatus, vault: Vec<Value>) -> Self {
            Self {
                status,
                vault,
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl BwCli for MockBw {
        fn status(&self) -> Result<BwStatus> {
            self.calls.borrow_mut().push("status".into());
            Ok(self.status.clone())
        }
        fn login_apikey(&self, _id: &str, _sec: &str) -> Result<()> {
            self.calls.borrow_mut().push("login".into());
            Ok(())
        }
        fn unlock(&self, _password: &str) -> Result<String> {
            self.calls.borrow_mut().push("unlock".into());
            Ok("MOCK_SESSION".into())
        }
        fn sync(&self, _session: &str) -> Result<()> {
            self.calls.borrow_mut().push("sync".into());
            Ok(())
        }
        fn get_item(&self, _session: &str, identifier: &str) -> Result<Value> {
            self.calls
                .borrow_mut()
                .push(format!("get_item:{identifier}"));
            self.vault
                .iter()
                .find(|i| i["id"].as_str() == Some(identifier))
                .cloned()
                .ok_or_else(|| anyhow!("Not found."))
        }
        fn list_items(
            &self,
            _session: &str,
            collection_id: Option<&str>,
            organization_id: Option<&str>,
        ) -> Result<Vec<SourceItem>> {
            self.calls.borrow_mut().push(format!(
                "list_items:{}:{}",
                collection_id.unwrap_or("-"),
                organization_id.unwrap_or("-")
            ));
            Ok(self
                .vault
                .iter()
                .map(|i| SourceItem {
                    name: i["name"].as_str().unwrap_or_default().into(),
                    id: i["id"].as_str().unwrap_or_default().into(),
                })
                .collect())
        }
    }

    /// A vault login item whose password is `password`.
    fn login(id: &str, name: &str, password: &str) -> Value {
        json!({"id": id, "name": name, "login": {"password": password}})
    }

    fn secret(name: &str, item: Option<&str>) -> ManifestSecret {
        ManifestSecret {
            name: name.into(),
            item: item.map(String::from),
            field: None,
            destination_names: Vec::new(),
        }
    }

    fn session_source(vault: Vec<Value>) -> BitwardenSource<MockBw> {
        BitwardenSource::with_cli(
            BitwardenSourceConfig::default(),
            MockBw::new(BwStatus::Unlocked, vault),
            BitwardenCredentials {
                session: Some("S".into()),
                ..Default::default()
            },
        )
    }

    fn bw_ref(item: &str, id: &str) -> SourceRef {
        SourceRef {
            source: "bitwarden".into(),
            item: item.into(),
            id: id.into(),
        }
    }

    fn saved_refs(entries: &[(&str, SourceRef)]) -> HashMap<String, SourceRef> {
        entries
            .iter()
            .map(|(n, r)| (n.to_string(), r.clone()))
            .collect()
    }

    /// Drains the recorded calls, so a test can assert one fetch at a time.
    fn take_calls(src: &BitwardenSource<MockBw>) -> Vec<String> {
        src.cli.calls.take()
    }

    /// The full error chain, as the CLI prints it.
    fn chain(err: &anyhow::Error) -> String {
        format!("{err:#}")
    }

    fn assert_no_value(err: &str, vault: &[Value]) {
        for item in vault {
            let value = item["login"]["password"].as_str().unwrap();
            assert!(!err.contains(value), "error leaked {value:?}: {err}");
        }
    }

    /// The vault from issue #80: the exact item beside two that contain it.
    fn issue_80_vault() -> Vec<Value> {
        vec![
            login("id-alt", "MY_TOKEN (alt)", "alt-value"),
            login("id-exact", "MY_TOKEN", "exact-value"),
            login("id-other", "MY_TOKEN (other account)", "other-value"),
        ]
    }

    #[test]
    fn bitwarden_source_logs_in_then_fetches() {
        let mock = MockBw::new(
            BwStatus::Unauthenticated,
            vec![login("id-stripe", "stripe", "sk")],
        );
        std::env::set_var(BW_CLIENTID_ENV, "id");
        std::env::set_var(BW_CLIENTSECRET_ENV, "sec");
        std::env::set_var(BW_PASSWORD_ENV, "pw");
        std::env::remove_var(BW_SESSION_ENV);
        let creds = BitwardenCredentials::from_env();
        let src = BitwardenSource::with_cli(BitwardenSourceConfig::default(), mock, creds);
        let got = src
            .fetch(&[secret("STRIPE", Some("stripe"))], &HashMap::new())
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "STRIPE");
        assert_eq!(got[0].value, "sk");
        assert_eq!(
            take_calls(&src),
            vec![
                "status",
                "login",
                "unlock",
                "sync",
                "list_items:-:-",
                "get_item:id-stripe"
            ]
        );
    }

    #[test]
    fn exact_name_resolves_beside_names_that_contain_it() {
        let src = session_source(issue_80_vault());
        let got = src
            .fetch(&[secret("MY_TOKEN", None)], &HashMap::new())
            .unwrap();
        assert_eq!(got[0].value, "exact-value");
        assert_eq!(got[0].source_ref, Some(bw_ref("MY_TOKEN", "id-exact")));
        assert_eq!(
            take_calls(&src),
            vec!["list_items:-:-", "get_item:id-exact"]
        );
    }

    #[test]
    fn duplicate_exact_names_fail_naming_both() {
        let vault = vec![
            login("id-a", "MY_TOKEN", "value-a"),
            login("id-b", "MY_TOKEN", "value-b"),
            login("id-c", "MY_TOKEN (alt)", "value-c"),
        ];
        let src = session_source(vault.clone());
        let err = chain(
            &src.fetch(&[secret("MY_TOKEN", None)], &HashMap::new())
                .unwrap_err(),
        );
        assert!(err.contains("fetching 'MY_TOKEN' from Bitwarden"), "{err}");
        assert!(err.contains("2 items match 'MY_TOKEN' exactly"), "{err}");
        assert!(err.contains("MY_TOKEN  (id-a)"), "{err}");
        assert!(err.contains("MY_TOKEN  (id-b)"), "{err}");
        assert!(
            !err.contains("id-c"),
            "only exact matches are candidates: {err}"
        );
        assert!(
            err.contains("set `item` to the intended item's ID"),
            "{err}"
        );
        assert_no_value(&err, &vault);
        // Never fetched a value it could not pin to one item.
        assert_eq!(take_calls(&src), vec!["list_items:-:-"]);
    }

    #[test]
    fn partial_and_case_only_matches_fail_listing_candidates() {
        let vault = vec![
            login("id-lower", "my_token", "lower-value"),
            login("id-alt", "MY_TOKEN (alt)", "alt-value"),
            login("id-unrelated", "OTHER", "unrelated-value"),
        ];
        let src = session_source(vault.clone());
        let err = chain(
            &src.fetch(&[secret("TOKEN", Some("MY_TOKEN"))], &HashMap::new())
                .unwrap_err(),
        );
        assert!(err.contains("fetching 'TOKEN' from Bitwarden"), "{err}");
        assert!(err.contains("no item is named exactly 'MY_TOKEN'"), "{err}");
        assert!(err.contains("my_token  (id-lower)"), "{err}");
        assert!(err.contains("MY_TOKEN (alt)  (id-alt)"), "{err}");
        assert!(!err.contains("id-unrelated"), "{err}");
        assert!(
            err.contains("set `item` to the exact name or the ID"),
            "{err}"
        );
        assert_no_value(&err, &vault);
        assert_eq!(take_calls(&src), vec!["list_items:-:-"]);
    }

    #[test]
    fn no_match_at_all_points_at_source_list() {
        let vault = vec![login("id-1", "OTHER", "other-value")];
        let src = session_source(vault.clone());
        let err = chain(
            &src.fetch(&[secret("MY_TOKEN", None)], &HashMap::new())
                .unwrap_err(),
        );
        assert!(err.contains("no item matched it"), "{err}");
        assert!(err.contains("gh-secrets source list"), "{err}");
        assert_no_value(&err, &vault);
    }

    #[test]
    fn candidate_lists_are_capped() {
        let vault: Vec<Value> = (0..25)
            .map(|i| {
                login(
                    &format!("id-{i}"),
                    &format!("MY_TOKEN {i}"),
                    &format!("capped-value-{i}"),
                )
            })
            .collect();
        let src = session_source(vault.clone());
        let err = chain(
            &src.fetch(&[secret("MY_TOKEN", None)], &HashMap::new())
                .unwrap_err(),
        );
        assert_eq!(err.matches("  (id-").count(), 20, "{err}");
        assert!(err.contains("and 5 more"), "{err}");
        assert_no_value(&err, &vault);
    }

    #[test]
    fn id_identifier_wins_over_an_item_named_that_id() {
        let vault = vec![
            login("id-target", "Real item", "target-value"),
            login("id-decoy", "id-target", "decoy-value"),
        ];
        let src = session_source(vault);
        let got = src
            .fetch(&[secret("TOKEN", Some("id-target"))], &HashMap::new())
            .unwrap();
        assert_eq!(got[0].value, "target-value");
        let saved_ref = got[0].source_ref.clone().unwrap();
        assert_eq!(saved_ref, bw_ref("id-target", "id-target"));
        assert_eq!(
            take_calls(&src),
            vec!["list_items:-:-", "get_item:id-target"]
        );

        // Every repeated fetch with the saved ref goes straight to the id.
        let saved = saved_refs(&[("TOKEN", saved_ref)]);
        for _ in 0..2 {
            let got = src
                .fetch(&[secret("TOKEN", Some("id-target"))], &saved)
                .unwrap();
            assert_eq!(got[0].value, "target-value");
            assert_eq!(got[0].source_ref, Some(bw_ref("id-target", "id-target")));
            assert_eq!(take_calls(&src), vec!["get_item:id-target"]);
        }
    }

    #[test]
    fn valid_saved_ref_fetches_by_id_without_listing() {
        let src = session_source(issue_80_vault());
        let saved = saved_refs(&[("MY_TOKEN", bw_ref("MY_TOKEN", "id-exact"))]);
        let got = src.fetch(&[secret("MY_TOKEN", None)], &saved).unwrap();
        assert_eq!(got[0].value, "exact-value");
        assert_eq!(got[0].source_ref, Some(bw_ref("MY_TOKEN", "id-exact")));
        assert_eq!(take_calls(&src), vec!["get_item:id-exact"]);
    }

    #[test]
    fn stale_saved_refs_fall_back_to_one_listing_and_rewrite_the_ref() {
        // (case, saved ref): an id that no longer fetches; an id whose item is
        // now named differently (a hand-edited id pointing elsewhere); a ref
        // for an identifier the manifest no longer uses; a ref another source
        // wrote.
        let cases = [
            (
                "deleted",
                bw_ref("MY_TOKEN", "id-gone"),
                vec!["get_item:id-gone"],
            ),
            (
                "elsewhere",
                bw_ref("MY_TOKEN", "id-alt"),
                vec!["get_item:id-alt"],
            ),
            ("old identifier", bw_ref("OLD_NAME", "id-alt"), vec![]),
            (
                "other source",
                SourceRef {
                    source: "env_file".into(),
                    ..bw_ref("MY_TOKEN", "id-alt")
                },
                vec![],
            ),
        ];
        for (case, stale, first_calls) in cases {
            let src = session_source(issue_80_vault());
            let saved = saved_refs(&[("MY_TOKEN", stale)]);
            let got = src.fetch(&[secret("MY_TOKEN", None)], &saved).unwrap();
            assert_eq!(got[0].value, "exact-value", "{case}");
            assert_eq!(
                got[0].source_ref,
                Some(bw_ref("MY_TOKEN", "id-exact")),
                "{case}"
            );
            let mut expected: Vec<&str> = first_calls;
            expected.extend(["list_items:-:-", "get_item:id-exact"]);
            assert_eq!(take_calls(&src), expected, "{case}");
        }
    }

    #[test]
    fn renamed_item_with_no_exact_match_fails_with_candidates() {
        // The saved item was renamed; nothing else is named MY_TOKEN exactly.
        let vault = vec![login("id-exact", "MY_TOKEN (renamed)", "renamed-value")];
        let src = session_source(vault.clone());
        let saved = saved_refs(&[("MY_TOKEN", bw_ref("MY_TOKEN", "id-exact"))]);
        let err = chain(&src.fetch(&[secret("MY_TOKEN", None)], &saved).unwrap_err());
        assert!(err.contains("no item is named exactly 'MY_TOKEN'"), "{err}");
        assert!(err.contains("MY_TOKEN (renamed)  (id-exact)"), "{err}");
        assert_no_value(&err, &vault);
        assert_eq!(
            take_calls(&src),
            vec!["get_item:id-exact", "list_items:-:-"]
        );
    }

    #[test]
    fn several_secrets_share_one_listing() {
        let mut vault = issue_80_vault();
        vault.push(login("id-db", "DB_URL", "db-value"));
        let src = session_source(vault);
        let got = src
            .fetch(
                &[
                    secret("MY_TOKEN", None),
                    secret("DATABASE_URL", Some("DB_URL")),
                    secret("ALT", Some("MY_TOKEN (alt)")),
                ],
                &HashMap::new(),
            )
            .unwrap();
        let values: Vec<&str> = got.iter().map(|f| f.value.as_str()).collect();
        assert_eq!(values, vec!["exact-value", "db-value", "alt-value"]);
        assert_eq!(
            take_calls(&src),
            vec![
                "list_items:-:-",
                "get_item:id-exact",
                "get_item:id-db",
                "get_item:id-alt"
            ]
        );
    }

    #[test]
    fn env_first_prefers_canonical_then_alias_and_skips_empty() {
        std::env::remove_var(BW_PASSWORD_ENV);
        std::env::remove_var("BITWARDEN_MASTER_PASSWORD");
        assert_eq!(env_first(BW_PASSWORD_ENVS), None);
        std::env::set_var("BITWARDEN_MASTER_PASSWORD", "alias");
        assert_eq!(env_first(BW_PASSWORD_ENVS).as_deref(), Some("alias"));
        // An empty canonical var is treated as unset, so the alias still wins.
        std::env::set_var(BW_PASSWORD_ENV, "");
        assert_eq!(env_first(BW_PASSWORD_ENVS).as_deref(), Some("alias"));
        // A non-empty canonical var takes priority over the alias.
        std::env::set_var(BW_PASSWORD_ENV, "canonical");
        assert_eq!(env_first(BW_PASSWORD_ENVS).as_deref(), Some("canonical"));
        std::env::remove_var(BW_PASSWORD_ENV);
        std::env::remove_var("BITWARDEN_MASTER_PASSWORD");
    }

    #[test]
    fn or_stored_fills_only_missing_fields_and_never_session() {
        // Env provided only the client id; stored config provides the rest.
        let from_env = BitwardenCredentials {
            client_id: Some("env-id".into()),
            session: Some("env-session".into()),
            ..Default::default()
        };
        let merged =
            from_env.or_stored(Some("stored-id"), Some("stored-secret"), Some("stored-pw"));
        // Env wins for client id...
        assert_eq!(merged.client_id.as_deref(), Some("env-id"));
        // ...stored fills the gaps...
        assert_eq!(merged.client_secret.as_deref(), Some("stored-secret"));
        assert_eq!(merged.password.as_deref(), Some("stored-pw"));
        // ...and session is left exactly as the env had it (never from stored).
        assert_eq!(merged.session.as_deref(), Some("env-session"));
        // An empty stored value counts as unset.
        let empty = BitwardenCredentials::default().or_stored(Some(""), None, None);
        assert_eq!(empty.client_id, None);
    }

    #[test]
    fn bitwarden_source_accepts_bitwarden_prefixed_aliases() {
        let mock = MockBw::new(BwStatus::Unauthenticated, vec![login("id-foo", "foo", "v")]);
        // Only the BITWARDEN_* aliases are set; the native BW_* names are unset.
        std::env::remove_var(BW_CLIENTID_ENV);
        std::env::remove_var(BW_CLIENTSECRET_ENV);
        std::env::remove_var(BW_PASSWORD_ENV);
        std::env::remove_var(BW_SESSION_ENV);
        std::env::set_var("BITWARDEN_CLIENT_ID", "id");
        std::env::set_var("BITWARDEN_CLIENT_SECRET", "sec");
        std::env::set_var("BITWARDEN_MASTER_PASSWORD", "pw");
        let creds = BitwardenCredentials::from_env();
        let src = BitwardenSource::with_cli(BitwardenSourceConfig::default(), mock, creds);
        let got = src
            .fetch(&[secret("FOO", Some("foo"))], &HashMap::new())
            .unwrap();
        assert_eq!(got[0].value, "v");
        assert_eq!(
            take_calls(&src),
            vec![
                "status",
                "login",
                "unlock",
                "sync",
                "list_items:-:-",
                "get_item:id-foo"
            ]
        );
        std::env::remove_var("BITWARDEN_CLIENT_ID");
        std::env::remove_var("BITWARDEN_CLIENT_SECRET");
        std::env::remove_var("BITWARDEN_MASTER_PASSWORD");
    }

    #[test]
    fn bitwarden_source_skips_login_when_session_override() {
        let src = session_source(vec![login("id-foo", "foo", "v")]);
        let got = src
            .fetch(&[secret("FOO", Some("foo"))], &HashMap::new())
            .unwrap();
        assert_eq!(got[0].value, "v");
        // No status/login/unlock/sync — straight to the lookups.
        assert_eq!(take_calls(&src), vec!["list_items:-:-", "get_item:id-foo"]);
    }

    #[test]
    fn bitwarden_source_lists_available_items_with_scope() {
        let mock = MockBw::new(
            BwStatus::Unlocked,
            vec![
                login("id-1", "Stripe API", "v1"),
                login("id-2", "GitHub PAT", "v2"),
            ],
        );
        let config = BitwardenSourceConfig {
            collection_id: Some("coll-1".into()),
            organization_id: Some("org-1".into()),
            default_field: None,
        };
        let src = BitwardenSource::with_cli(
            config,
            mock,
            BitwardenCredentials {
                session: Some("S".into()),
                ..Default::default()
            },
        );
        let got = src.list_available().unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "Stripe API");
        assert_eq!(got[0].id, "id-1");
        // Session short-circuits auth; the collection/org scope is forwarded.
        assert_eq!(take_calls(&src), vec!["list_items:coll-1:org-1"]);
    }

    #[test]
    fn env_file_source_reads_keys_and_rejects_fields() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".env.master");
        std::fs::write(&path, "FOO=\"bar\"\nDB_URL=postgres://x\n").unwrap();
        let src = EnvFileSource { path };
        // `item` maps the secret name onto a different key in the file.
        let got = src
            .fetch(&[secret("DATABASE_URL", Some("DB_URL"))], &HashMap::new())
            .unwrap();
        assert_eq!(got[0].value, "postgres://x");
        assert_eq!(got[0].source_ref, None);
        // A Bitwarden-style field selector is a config error here.
        let err = src
            .fetch(
                &[ManifestSecret {
                    field: Some("password".into()),
                    ..secret("FOO", None)
                }],
                &HashMap::new(),
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("has no fields"), "got: {err}");
        // Listing surfaces keys, never values.
        let mut items = src.list_available().unwrap();
        items.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(items[0].name, "DB_URL");
        assert_eq!(items[1].name, "FOO");
    }

    #[test]
    fn local_store_source_reads_vault_secrets() {
        std::env::set_var(vault::PASSPHRASE_ENV, "test-pass");
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("vault.json");
        let mut data = vault::VaultData::default();
        data.secrets.insert("FOO".into(), "from-vault".into());
        vault::save(&path, &data).unwrap();
        let src = LocalStoreSource::new(&path);
        let got = src.fetch(&[secret("FOO", None)], &HashMap::new()).unwrap();
        assert_eq!(got[0].value, "from-vault");
        assert_eq!(got[0].source_ref, None);
        let items = src.list_available().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "FOO");
        std::env::remove_var(vault::PASSPHRASE_ENV);
    }

    #[test]
    fn static_source_lists_its_keys_as_items() {
        let src = StaticSource::new([("FOO", "v1"), ("BAR", "v2")]);
        let mut got = src.list_available().unwrap();
        got.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(
            got,
            vec![
                SourceItem {
                    name: "BAR".into(),
                    id: "BAR".into()
                },
                SourceItem {
                    name: "FOO".into(),
                    id: "FOO".into()
                },
            ]
        );
    }
}
