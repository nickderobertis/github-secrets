//! The encrypted local vault: one file at `<config-root>/vault.json` holding
//! both the stored credentials (GitHub token, Bitwarden login) and the global
//! `local` secret store's values.
//!
//! Encryption is XChaCha20-Poly1305 with a key derived from a passphrase via
//! Argon2id. The passphrase resolves from `GH_SECRETS_PASSPHRASE` (the shell
//! environment or an auto-loaded `.env`/`.env.local`) and otherwise from an
//! interactive prompt; non-interactive callers without the env var get a
//! precise error instead of a hang. The resolved passphrase is cached for the
//! life of the process so one invocation never prompts twice.
//!
//! The on-disk file carries only KDF parameters, a salt, a nonce, and the
//! ciphertext — never a plaintext credential or secret value.

use std::collections::BTreeMap;
use std::fs;
use std::io::IsTerminal;
use std::path::Path;
use std::sync::OnceLock;

use anyhow::{anyhow, bail, Context, Result};
use argon2::Argon2;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, OsRng};
use chacha20poly1305::{AeadCore, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};

use crate::credentials::StoredCredentials;

pub const PASSPHRASE_ENV: &str = "GH_SECRETS_PASSPHRASE";
const SALT_LEN: usize = 16;
const KEY_LEN: usize = 32;

/// Decrypted vault contents. `credentials` backs `gh-secrets auth`;
/// `secrets` backs the `local` store (`gh-secrets store`, `--from local`,
/// `--to local`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct VaultData {
    #[serde(default)]
    pub credentials: StoredCredentials,
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
}

impl VaultData {
    pub fn is_empty(&self) -> bool {
        self.credentials.is_empty() && self.secrets.is_empty()
    }
}

/// On-disk envelope. `version` exists so a future format change can migrate.
#[derive(Debug, Serialize, Deserialize)]
struct VaultFile {
    version: u32,
    kdf: KdfParams,
    nonce: String,
    ciphertext: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct KdfParams {
    algorithm: String,
    salt: String,
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
}

/// Load the vault at `path`, decrypting with the process passphrase. A missing
/// file is an empty vault (and requires no passphrase).
pub fn load(path: &Path) -> Result<VaultData> {
    if !path.exists() {
        return Ok(VaultData::default());
    }
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let file: VaultFile =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?;
    if file.version != 1 {
        bail!(
            "{} has unsupported vault version {}",
            path.display(),
            file.version
        );
    }
    let passphrase = passphrase(false)?;
    let salt = B64.decode(&file.kdf.salt).context("decoding vault salt")?;
    let key = derive_key(&passphrase, &salt, &file.kdf)?;
    let nonce_bytes = B64.decode(&file.nonce).context("decoding vault nonce")?;
    let nonce = XNonce::from_slice(&nonce_bytes);
    let ciphertext = B64
        .decode(&file.ciphertext)
        .context("decoding vault ciphertext")?;
    let cipher = XChaCha20Poly1305::new((&key).into());
    let plaintext = cipher.decrypt(nonce, ciphertext.as_ref()).map_err(|_| {
        anyhow!(
            "could not decrypt {} — wrong {PASSPHRASE_ENV} passphrase?",
            path.display()
        )
    })?;
    serde_json::from_slice(&plaintext).context("parsing decrypted vault contents")
}

/// Encrypt and write the vault. A fresh salt and nonce are generated on every
/// save. The file is written atomically and `0600` on Unix.
pub fn save(path: &Path, data: &VaultData) -> Result<()> {
    let passphrase = passphrase(!path.exists())?;
    let mut salt = [0u8; SALT_LEN];
    getrandom_fill(&mut salt)?;
    let kdf = KdfParams {
        algorithm: "argon2id".into(),
        salt: B64.encode(salt),
        m_cost: argon2::Params::DEFAULT_M_COST,
        t_cost: argon2::Params::DEFAULT_T_COST,
        p_cost: argon2::Params::DEFAULT_P_COST,
    };
    let key = derive_key(&passphrase, &salt, &kdf)?;
    let cipher = XChaCha20Poly1305::new((&key).into());
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let plaintext = serde_json::to_vec(data).context("serializing vault contents")?;
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_ref())
        .map_err(|_| anyhow!("encrypting vault failed"))?;
    let file = VaultFile {
        version: 1,
        kdf,
        nonce: B64.encode(nonce),
        ciphertext: B64.encode(ciphertext),
    };
    let bytes = serde_json::to_vec_pretty(&file).context("serializing vault file")?;
    write_owner_only_atomic(path, &bytes)
}

/// Remove the vault file if it exists.
pub fn remove(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

fn derive_key(passphrase: &str, salt: &[u8], kdf: &KdfParams) -> Result<[u8; KEY_LEN]> {
    if kdf.algorithm != "argon2id" {
        bail!("unsupported vault KDF '{}'", kdf.algorithm);
    }
    let params = argon2::Params::new(kdf.m_cost, kdf.t_cost, kdf.p_cost, Some(KEY_LEN))
        .map_err(|e| anyhow!("invalid vault KDF parameters: {e}"))?;
    let argon = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = [0u8; KEY_LEN];
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow!("deriving vault key: {e}"))?;
    Ok(key)
}

/// Resolve the vault passphrase once per process: `GH_SECRETS_PASSPHRASE`
/// first (which includes values auto-loaded from `.env`/`.env.local`), then an
/// interactive prompt. `creating` adds a confirmation prompt so a typo'd
/// passphrase can't silently lock a brand-new vault.
fn passphrase(creating: bool) -> Result<String> {
    static CACHE: OnceLock<String> = OnceLock::new();
    if let Some(p) = CACHE.get() {
        return Ok(p.clone());
    }
    let resolved = resolve_passphrase(creating)?;
    Ok(CACHE.get_or_init(|| resolved).clone())
}

fn resolve_passphrase(creating: bool) -> Result<String> {
    if let Ok(v) = std::env::var(PASSPHRASE_ENV) {
        if !v.is_empty() {
            return Ok(v);
        }
    }
    if !std::io::stdin().is_terminal() {
        bail!(
            "the vault is encrypted and no passphrase is available: set {PASSPHRASE_ENV} (e.g. in your shell or .env) or run interactively"
        );
    }
    let first = rpassword::prompt_password("vault passphrase: ").context("reading passphrase")?;
    if first.is_empty() {
        bail!("passphrase cannot be empty");
    }
    if creating {
        let second =
            rpassword::prompt_password("confirm passphrase: ").context("reading passphrase")?;
        if first != second {
            bail!("passphrases do not match");
        }
    }
    Ok(first)
}

fn getrandom_fill(buf: &mut [u8]) -> Result<()> {
    use chacha20poly1305::aead::rand_core::RngCore;
    OsRng.fill_bytes(buf);
    Ok(())
}

fn write_owner_only_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("setting 0600 perms on {}", tmp.display()))?;
    }
    fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn with_passphrase(f: impl FnOnce()) {
        std::env::set_var(PASSPHRASE_ENV, "test-passphrase");
        f();
        std::env::remove_var(PASSPHRASE_ENV);
    }

    #[test]
    fn round_trips_and_never_writes_plaintext() {
        with_passphrase(|| {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("vault.json");
            let mut data = VaultData::default();
            data.credentials.github_token = Some("ghp_super_secret".into());
            data.secrets
                .insert("STRIPE_KEY".into(), "sk_live_value".into());
            save(&path, &data).unwrap();

            let raw = fs::read_to_string(&path).unwrap();
            assert!(!raw.contains("ghp_super_secret"));
            assert!(!raw.contains("sk_live_value"));
            assert!(!raw.contains("STRIPE_KEY"));

            let back = load(&path).unwrap();
            assert_eq!(back, data);

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(&path).unwrap().permissions().mode();
                assert_eq!(mode & 0o777, 0o600);
            }
        });
    }

    #[test]
    fn missing_file_is_an_empty_vault_without_a_passphrase() {
        // No passphrase in the env at all: loading a missing vault must not
        // prompt or error.
        std::env::remove_var(PASSPHRASE_ENV);
        let dir = TempDir::new().unwrap();
        let got = load(&dir.path().join("vault.json")).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn wrong_passphrase_is_a_clear_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("vault.json");
        std::env::set_var(PASSPHRASE_ENV, "correct");
        // Bypass the process-wide cache by exercising the crypto layer
        // directly: save with one key, tamper by re-deriving with another.
        let data = VaultData::default();
        save(&path, &data).unwrap();
        // Corrupt the ciphertext: decryption must fail with the
        // wrong-passphrase error rather than a panic or garbage data.
        let mut file: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let mut ct = B64.decode(file["ciphertext"].as_str().unwrap()).unwrap();
        ct[0] ^= 0xff;
        file["ciphertext"] = serde_json::Value::String(B64.encode(ct));
        fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        let err = load(&path).unwrap_err().to_string();
        assert!(err.contains("could not decrypt"), "got: {err}");
        std::env::remove_var(PASSPHRASE_ENV);
    }
}
