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

/// Anything that can hand the orchestrator the current values for a list of
/// manifest secrets.
pub trait SecretSource {
    fn fetch(&self, secrets: &[ManifestSecret]) -> Result<Vec<FetchedSecret>>;
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
}

// ---- Bitwarden ----

pub const BW_CLIENTID_ENV: &str = "BW_CLIENTID";
pub const BW_CLIENTSECRET_ENV: &str = "BW_CLIENTSECRET";
pub const BW_PASSWORD_ENV: &str = "BW_PASSWORD";
pub const BW_SESSION_ENV: &str = "BW_SESSION";

/// Thin layer over the `bw` CLI. Exists so the field-extraction and error
/// paths can be unit-tested without the binary on `$PATH`.
pub trait BwCli {
    fn status(&self) -> Result<BwStatus>;
    fn login_apikey(&self, client_id: &str, client_secret: &str) -> Result<()>;
    fn unlock(&self, password: &str) -> Result<String>;
    fn sync(&self, session: &str) -> Result<()>;
    fn get_item(&self, session: &str, identifier: &str) -> Result<Value>;
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
}

pub struct BitwardenSource<C: BwCli = RealBwCli> {
    config: BitwardenSourceConfig,
    cli: C,
    /// If set, used instead of running login/unlock. Useful when the caller
    /// already has an unlocked session.
    session_override: Option<String>,
}

impl BitwardenSource<RealBwCli> {
    pub fn new(config: BitwardenSourceConfig) -> Self {
        Self {
            config,
            cli: RealBwCli,
            session_override: env::var(BW_SESSION_ENV).ok().filter(|s| !s.is_empty()),
        }
    }
}

impl<C: BwCli> BitwardenSource<C> {
    pub fn with_cli(config: BitwardenSourceConfig, cli: C, session: Option<String>) -> Self {
        Self {
            config,
            cli,
            session_override: session,
        }
    }

    fn ensure_session(&self) -> Result<String> {
        if let Some(s) = &self.session_override {
            return Ok(s.clone());
        }
        let status = self.cli.status().context("checking `bw status`")?;
        if status == BwStatus::Unauthenticated {
            let client_id = env::var(BW_CLIENTID_ENV)
                .map_err(|_| anyhow!("{BW_CLIENTID_ENV} must be set to log in to Bitwarden"))?;
            let client_secret = env::var(BW_CLIENTSECRET_ENV)
                .map_err(|_| anyhow!("{BW_CLIENTSECRET_ENV} must be set to log in to Bitwarden"))?;
            self.cli
                .login_apikey(&client_id, &client_secret)
                .context("`bw login --apikey` failed")?;
        }
        let password = env::var(BW_PASSWORD_ENV)
            .map_err(|_| anyhow!("{BW_PASSWORD_ENV} must be set to unlock the Bitwarden vault"))?;
        let session = self.cli.unlock(&password).context("`bw unlock` failed")?;
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
    }

    #[test]
    fn bitwarden_source_logs_in_then_fetches() {
        let mut items = HashMap::new();
        items.insert("stripe".to_string(), json!({"login": {"password": "sk"}}));
        let mock = MockBw {
            status: BwStatus::Unauthenticated,
            items,
            calls: RefCell::new(Vec::new()),
        };
        std::env::set_var(BW_CLIENTID_ENV, "id");
        std::env::set_var(BW_CLIENTSECRET_ENV, "sec");
        std::env::set_var(BW_PASSWORD_ENV, "pw");
        std::env::remove_var(BW_SESSION_ENV);
        let src = BitwardenSource::with_cli(BitwardenSourceConfig::default(), mock, None);
        let secrets = vec![ManifestSecret {
            name: "STRIPE".into(),
            item: Some("stripe".into()),
            field: None,
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
    fn bitwarden_source_skips_login_when_session_override() {
        let mut items = HashMap::new();
        items.insert("foo".to_string(), json!({"login": {"password": "v"}}));
        let mock = MockBw {
            status: BwStatus::Unlocked,
            items,
            calls: RefCell::new(Vec::new()),
        };
        let src = BitwardenSource::with_cli(
            BitwardenSourceConfig::default(),
            mock,
            Some("ABCDEF".into()),
        );
        let secrets = vec![ManifestSecret {
            name: "FOO".into(),
            item: Some("foo".into()),
            field: None,
        }];
        let got = src.fetch(&secrets).unwrap();
        assert_eq!(got[0].value, "v");
        let calls = src.cli.calls.borrow().clone();
        // No status/login/unlock/sync — straight to get_item.
        assert_eq!(calls, vec!["get_item:foo"]);
    }
}
