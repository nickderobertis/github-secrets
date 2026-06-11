//! Secret sources: where the manifest pulls values from.
//!
//! `SecretSource` is the single-method trait the orchestrator drives. The
//! first concrete implementation is `BitwardenSource`, which shells out to
//! the official `bw` CLI authenticated via the Bitwarden personal API key
//! (`BW_CLIENTID` + `BW_CLIENTSECRET` + `BW_PASSWORD`).

use std::collections::HashMap;
use std::env;
use std::process::{Command, Output};

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::manifest::{BitwardenSourceConfig, ManifestSecret};

/// A plaintext value retrieved from a source. Callers MUST treat this as
/// sensitive and must never log or persist `value` outside the documented
/// destinations.
#[derive(Debug, Clone)]
pub struct FetchedSecret {
    pub name: String,
    pub value: String,
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
    fn fetch(&self, secrets: &[ManifestSecret]) -> Result<Vec<FetchedSecret>>;

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
    fn fetch(&self, secrets: &[ManifestSecret]) -> Result<Vec<FetchedSecret>> {
        let mut out = Vec::with_capacity(secrets.len());
        for s in secrets {
            let value = self
                .values
                .get(&s.name)
                .ok_or_else(|| anyhow!("static source has no value for '{}'", s.name))?;
            out.push(FetchedSecret {
                name: s.name.clone(),
                value: value.clone(),
            });
        }
        Ok(out)
    }

    fn list_available(&self) -> Result<Vec<SourceItem>> {
        // No real ids in a static map, so the key doubles as name and id.
        Ok(self
            .values
            .keys()
            .map(|k| SourceItem {
                name: k.clone(),
                id: k.clone(),
            })
            .collect())
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
}

impl<C: BwCli> SecretSource for BitwardenSource<C> {
    fn fetch(&self, secrets: &[ManifestSecret]) -> Result<Vec<FetchedSecret>> {
        if secrets.is_empty() {
            return Ok(Vec::new());
        }
        let session = self.ensure_session()?;
        let mut out = Vec::with_capacity(secrets.len());
        for s in secrets {
            let item_id = s.source_item();
            let item = self
                .cli
                .get_item(&session, item_id)
                .with_context(|| format!("fetching '{}' from Bitwarden", s.name))?;
            let field = s.field.as_deref().unwrap_or_else(|| self.default_field());
            let value = extract_field(&item, field).with_context(|| {
                format!("extracting field '{field}' from Bitwarden item '{item_id}'")
            })?;
            out.push(FetchedSecret {
                name: s.name.clone(),
                value,
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
        other => Err(anyhow!("unsupported field selector '{other}'")),
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
        assert!(extract_field(&item, "totp").is_err());
    }

    /// Mock CLI that tracks calls so we can assert the source did login/unlock
    /// in the right order and used the session token for subsequent calls.
    struct MockBw {
        status: BwStatus,
        items: HashMap<String, Value>,
        listed: Vec<SourceItem>,
        calls: RefCell<Vec<String>>,
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
            self.items
                .get(identifier)
                .cloned()
                .ok_or_else(|| anyhow!("no mock item '{identifier}'"))
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
            Ok(self.listed.clone())
        }
    }

    #[test]
    fn bitwarden_source_logs_in_then_fetches() {
        let mut items = HashMap::new();
        items.insert("stripe".to_string(), json!({"login": {"password": "sk"}}));
        let mock = MockBw {
            status: BwStatus::Unauthenticated,
            items,
            listed: Vec::new(),
            calls: RefCell::new(Vec::new()),
        };
        std::env::set_var(BW_CLIENTID_ENV, "id");
        std::env::set_var(BW_CLIENTSECRET_ENV, "sec");
        std::env::set_var(BW_PASSWORD_ENV, "pw");
        std::env::remove_var(BW_SESSION_ENV);
        let creds = BitwardenCredentials::from_env();
        let src = BitwardenSource::with_cli(BitwardenSourceConfig::default(), mock, creds);
        let secrets = vec![ManifestSecret {
            name: "STRIPE".into(),
            item: Some("stripe".into()),
            field: None,
            destination_names: Vec::new(),
        }];
        let got = src.fetch(&secrets).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "STRIPE");
        assert_eq!(got[0].value, "sk");
        let calls = src.cli.calls.borrow().clone();
        assert_eq!(
            calls,
            vec!["status", "login", "unlock", "sync", "get_item:stripe"]
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
        let mut items = HashMap::new();
        items.insert("foo".to_string(), json!({"login": {"password": "v"}}));
        let mock = MockBw {
            status: BwStatus::Unauthenticated,
            items,
            listed: Vec::new(),
            calls: RefCell::new(Vec::new()),
        };
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
        let secrets = vec![ManifestSecret {
            name: "FOO".into(),
            item: Some("foo".into()),
            field: None,
            destination_names: Vec::new(),
        }];
        let got = src.fetch(&secrets).unwrap();
        assert_eq!(got[0].value, "v");
        let calls = src.cli.calls.borrow().clone();
        assert_eq!(
            calls,
            vec!["status", "login", "unlock", "sync", "get_item:foo"]
        );
        std::env::remove_var("BITWARDEN_CLIENT_ID");
        std::env::remove_var("BITWARDEN_CLIENT_SECRET");
        std::env::remove_var("BITWARDEN_MASTER_PASSWORD");
    }

    #[test]
    fn bitwarden_source_skips_login_when_session_override() {
        let mut items = HashMap::new();
        items.insert("foo".to_string(), json!({"login": {"password": "v"}}));
        let mock = MockBw {
            status: BwStatus::Unlocked,
            items,
            listed: Vec::new(),
            calls: RefCell::new(Vec::new()),
        };
        let src = BitwardenSource::with_cli(
            BitwardenSourceConfig::default(),
            mock,
            BitwardenCredentials {
                session: Some("ABCDEF".into()),
                ..Default::default()
            },
        );
        let secrets = vec![ManifestSecret {
            name: "FOO".into(),
            item: Some("foo".into()),
            field: None,
            destination_names: Vec::new(),
        }];
        let got = src.fetch(&secrets).unwrap();
        assert_eq!(got[0].value, "v");
        let calls = src.cli.calls.borrow().clone();
        // No status/login/unlock/sync — straight to get_item.
        assert_eq!(calls, vec!["get_item:foo"]);
    }

    #[test]
    fn bitwarden_source_lists_available_items_with_scope() {
        let mock = MockBw {
            status: BwStatus::Unlocked,
            items: HashMap::new(),
            listed: vec![
                SourceItem {
                    name: "Stripe API".into(),
                    id: "id-1".into(),
                },
                SourceItem {
                    name: "GitHub PAT".into(),
                    id: "id-2".into(),
                },
            ],
            calls: RefCell::new(Vec::new()),
        };
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
        let calls = src.cli.calls.borrow().clone();
        assert_eq!(calls, vec!["list_items:coll-1:org-1"]);
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
