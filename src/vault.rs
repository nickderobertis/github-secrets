//! The encrypted local vault: one file at `<config-root>/vault.json` holding
//! both the stored credentials (GitHub token, Bitwarden login) and the global
//! `local` secret store's values.
//!
//! Envelope encryption: the vault data is encrypted with a random 32-byte
//! *data key* (XChaCha20-Poly1305), and the data key is stored alongside it
//! wrapped by a *KEK* derived from the passphrase via Argon2id. The
//! passphrase is therefore never persisted anywhere, and unlocking can be
//! cached without it: a successful unlock can mint a time-boxed **session**
//! (`session.json` next to the vault, `0600`) holding the data key, so
//! follow-up invocations within the session window need no passphrase at all
//! — the same tradeoff as `bw unlock` / a sudo timestamp, bounded by file
//! permissions and the expiry. `gh-secrets auth lock` (or expiry) deletes it.
//!
//! Unlock order: active session > `GH_SECRETS_PASSPHRASE` (shell environment
//! or auto-loaded `.env`/`.env.local`) > interactive prompt. A prompt-based
//! unlock starts a session automatically (telling the user on stderr);
//! non-interactive callers without any of the three get a precise error
//! instead of a hang. The resolved passphrase is cached for the life of the
//! process so one invocation never prompts twice.
//!
//! The on-disk vault carries only KDF parameters, a salt, nonces, and
//! ciphertexts — never a plaintext credential or secret value.

use std::collections::BTreeMap;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use argon2::Argon2;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, OsRng};
use chacha20poly1305::{AeadCore, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};

use crate::credentials::StoredCredentials;

pub const PASSPHRASE_ENV: &str = "GH_SECRETS_PASSPHRASE";
pub const DEFAULT_SESSION_DAYS: u64 = 7;
const SESSION_FILE: &str = "session.json";
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
    /// The data key, encrypted with the passphrase-derived KEK.
    wrapped_key: Sealed,
    /// The serialized [`VaultData`], encrypted with the data key.
    data: Sealed,
}

#[derive(Debug, Serialize, Deserialize)]
struct KdfParams {
    algorithm: String,
    salt: String,
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
}

/// One AEAD ciphertext with its nonce, both base64.
#[derive(Debug, Serialize, Deserialize)]
struct Sealed {
    nonce: String,
    ciphertext: String,
}

/// Plaintext session file next to the vault: the data key plus a hard expiry
/// (unix seconds). Holding this file IS holding the vault key for its
/// lifetime — that is the deliberate convenience/security tradeoff, bounded
/// by `0600` permissions and the expiry.
#[derive(Debug, Serialize, Deserialize)]
struct SessionFile {
    version: u32,
    key: String,
    expires_at: u64,
}

type Key = [u8; KEY_LEN];

/// Load the vault at `path`, decrypting via session or passphrase. A missing
/// file is an empty vault (and requires no unlock).
pub fn load(path: &Path) -> Result<VaultData> {
    if !path.exists() {
        return Ok(VaultData::default());
    }
    let unlocked = unlock(path)?;
    serde_json::from_slice(&unlocked.plaintext).context("parsing decrypted vault contents")
}

/// Encrypt and write the vault. An existing vault keeps its KDF salt and
/// wrapped key (so a session alone suffices to write); a brand-new vault
/// derives everything fresh from the passphrase. The file is written
/// atomically and `0600` on Unix.
pub fn save(path: &Path, data: &VaultData) -> Result<()> {
    let plaintext = serde_json::to_vec(data).context("serializing vault contents")?;
    if path.exists() {
        let mut unlocked = unlock(path)?;
        unlocked.file.data = seal(&unlocked.key, &plaintext)?;
        return write_vault_file(path, &unlocked.file);
    }

    // New vault: the passphrase is required (a session can't exist yet) and,
    // when prompted for, confirmed so a typo can't silently lock the vault.
    let (passphrase, prompted) = passphrase(true)?;
    let mut salt = [0u8; SALT_LEN];
    random_fill(&mut salt);
    let kdf = KdfParams {
        algorithm: "argon2id".into(),
        salt: B64.encode(salt),
        m_cost: argon2::Params::DEFAULT_M_COST,
        t_cost: argon2::Params::DEFAULT_T_COST,
        p_cost: argon2::Params::DEFAULT_P_COST,
    };
    let kek = derive_key(&passphrase, &salt, &kdf)?;
    let mut key: Key = [0u8; KEY_LEN];
    random_fill(&mut key);
    let file = VaultFile {
        version: 1,
        kdf,
        wrapped_key: seal(&kek, &key)?,
        data: seal(&key, &plaintext)?,
    };
    write_vault_file(path, &file)?;
    if prompted {
        start_session_with_key(path, &key, DEFAULT_SESSION_DAYS)?;
        announce_session(DEFAULT_SESSION_DAYS);
    }
    Ok(())
}

/// Remove the vault file and any session for it.
pub fn remove(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
    }
    end_session(path)?;
    Ok(())
}

// ---- sessions ----

/// Verify the passphrase and start (or refresh) a session valid for `days`.
/// Deliberately ignores any existing session: extending an unlock always
/// re-proves knowledge of the passphrase.
pub fn start_session(path: &Path, days: u64) -> Result<()> {
    if !path.exists() {
        bail!(
            "no vault at {} yet: store something first (`gh-secrets auth ...` or `gh-secrets store set ...`)",
            path.display()
        );
    }
    let file = read_vault_file(path)?;
    let key = unwrap_key_with_passphrase(path, &file)?.0;
    start_session_with_key(path, &key, days)
}

/// Delete the session if one exists. Returns whether one was removed.
pub fn end_session(path: &Path) -> Result<bool> {
    let session = session_path(path);
    if session.exists() {
        fs::remove_file(&session).with_context(|| format!("removing {}", session.display()))?;
        return Ok(true);
    }
    Ok(false)
}

/// Unix-seconds expiry of the active session, if any. An expired session is
/// deleted on read, so `Some` always means currently usable.
pub fn session_expiry(path: &Path) -> Option<u64> {
    read_session(path).map(|(_, expires_at)| expires_at)
}

fn start_session_with_key(path: &Path, key: &Key, days: u64) -> Result<()> {
    let expires_at = now_unix() + days * 24 * 60 * 60;
    let session = SessionFile {
        version: 1,
        key: B64.encode(key),
        expires_at,
    };
    let bytes = serde_json::to_vec_pretty(&session).context("serializing session")?;
    write_owner_only_atomic(&session_path(path), &bytes)
}

fn session_path(vault_path: &Path) -> PathBuf {
    vault_path.with_file_name(SESSION_FILE)
}

/// Read a usable session key. Expired or malformed sessions are deleted and
/// treated as absent.
fn read_session(vault_path: &Path) -> Option<(Key, u64)> {
    let path = session_path(vault_path);
    let bytes = fs::read(&path).ok()?;
    let parsed: Option<(Key, u64)> = serde_json::from_slice::<SessionFile>(&bytes)
        .ok()
        .filter(|s| s.version == 1 && s.expires_at > now_unix())
        .and_then(|s| {
            let raw = B64.decode(&s.key).ok()?;
            let key: Key = raw.as_slice().try_into().ok()?;
            Some((key, s.expires_at))
        });
    if parsed.is_none() {
        let _ = fs::remove_file(&path);
    }
    parsed
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

fn announce_session(days: u64) {
    eprintln!("vault unlocked for {days} day(s); run `gh-secrets auth lock` to forget early");
}

// ---- unlock ----

struct Unlocked {
    file: VaultFile,
    key: Key,
    plaintext: Vec<u8>,
}

/// Open an existing vault: session first, then passphrase. A prompt-based
/// passphrase unlock starts a session as a side effect.
fn unlock(path: &Path) -> Result<Unlocked> {
    let file = read_vault_file(path)?;
    if let Some((key, _)) = read_session(path) {
        if let Ok(plaintext) = open(&key, &file.data) {
            return Ok(Unlocked {
                file,
                key,
                plaintext,
            });
        }
        // The session key doesn't fit this vault (e.g. the vault was
        // recreated): a stale session is useless, drop it.
        let _ = end_session(path);
    }
    let (key, prompted) = unwrap_key_with_passphrase(path, &file)?;
    let plaintext = open(&key, &file.data).map_err(|_| {
        anyhow!(
            "{} is corrupt: the data section does not decrypt",
            path.display()
        )
    })?;
    if prompted {
        start_session_with_key(path, &key, DEFAULT_SESSION_DAYS)?;
        announce_session(DEFAULT_SESSION_DAYS);
    }
    Ok(Unlocked {
        file,
        key,
        plaintext,
    })
}

/// Resolve the passphrase, derive the KEK, and unwrap the data key. The
/// returned flag is true when the passphrase came from an interactive prompt.
fn unwrap_key_with_passphrase(path: &Path, file: &VaultFile) -> Result<(Key, bool)> {
    let (passphrase, prompted) = passphrase(false)?;
    let salt = B64.decode(&file.kdf.salt).context("decoding vault salt")?;
    let kek = derive_key(&passphrase, &salt, &file.kdf)?;
    let raw = open(&kek, &file.wrapped_key).map_err(|_| {
        anyhow!(
            "could not decrypt {} — wrong {PASSPHRASE_ENV} passphrase?",
            path.display()
        )
    })?;
    let key: Key = raw.as_slice().try_into().map_err(|_| {
        anyhow!(
            "{} is corrupt: wrapped key has wrong length",
            path.display()
        )
    })?;
    Ok((key, prompted))
}

fn read_vault_file(path: &Path) -> Result<VaultFile> {
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
    Ok(file)
}

fn write_vault_file(path: &Path, file: &VaultFile) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(file).context("serializing vault file")?;
    write_owner_only_atomic(path, &bytes)
}

// ---- crypto ----

fn seal(key: &Key, plaintext: &[u8]) -> Result<Sealed> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|_| anyhow!("encrypting vault failed"))?;
    Ok(Sealed {
        nonce: B64.encode(nonce),
        ciphertext: B64.encode(ciphertext),
    })
}

/// Decrypt one sealed section. The error is unit so callers attach the right
/// context (wrong passphrase vs. stale session vs. corruption).
fn open(key: &Key, sealed: &Sealed) -> std::result::Result<Vec<u8>, ()> {
    let nonce_bytes = B64.decode(&sealed.nonce).map_err(|_| ())?;
    let ciphertext = B64.decode(&sealed.ciphertext).map_err(|_| ())?;
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(XNonce::from_slice(&nonce_bytes), ciphertext.as_ref())
        .map_err(|_| ())
}

fn derive_key(passphrase: &str, salt: &[u8], kdf: &KdfParams) -> Result<Key> {
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
/// interactive prompt. The second tuple element is true when the user was
/// prompted (the signal to start a session). `creating` adds a confirmation
/// prompt so a typo'd passphrase can't silently lock a brand-new vault.
fn passphrase(creating: bool) -> Result<(String, bool)> {
    static CACHE: OnceLock<(String, bool)> = OnceLock::new();
    if let Some(cached) = CACHE.get() {
        return Ok(cached.clone());
    }
    let resolved = resolve_passphrase(creating)?;
    Ok(CACHE.get_or_init(|| resolved).clone())
}

fn resolve_passphrase(creating: bool) -> Result<(String, bool)> {
    if let Ok(v) = std::env::var(PASSPHRASE_ENV) {
        if !v.is_empty() {
            return Ok((v, false));
        }
    }
    if !std::io::stdin().is_terminal() {
        bail!(
            "the vault is locked and no passphrase is available: set {PASSPHRASE_ENV} (e.g. in your shell or .env), run `gh-secrets auth unlock` once from a terminal, or run interactively"
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
    Ok((first, true))
}

fn random_fill(buf: &mut [u8]) {
    use chacha20poly1305::aead::rand_core::RngCore;
    OsRng.fill_bytes(buf);
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

    fn sample_data() -> VaultData {
        let mut data = VaultData::default();
        data.credentials.github_token = Some("ghp_super_secret".into());
        data.secrets
            .insert("STRIPE_KEY".into(), "sk_live_value".into());
        data
    }

    #[test]
    fn round_trips_and_never_writes_plaintext() {
        with_passphrase(|| {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("vault.json");
            let data = sample_data();
            save(&path, &data).unwrap();

            let raw = fs::read_to_string(&path).unwrap();
            assert!(!raw.contains("ghp_super_secret"));
            assert!(!raw.contains("sk_live_value"));
            assert!(!raw.contains("STRIPE_KEY"));
            assert!(!raw.contains("test-passphrase"));

            let back = load(&path).unwrap();
            assert_eq!(back, data);

            // An env-passphrase unlock must not silently start a session.
            assert!(session_expiry(&path).is_none());

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
        save(&path, &VaultData::default()).unwrap();
        // Corrupt the wrapped key: with the (process-cached) passphrase the
        // unwrap now fails exactly as a wrong passphrase would.
        let mut file: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let mut ct = B64
            .decode(file["wrapped_key"]["ciphertext"].as_str().unwrap())
            .unwrap();
        ct[0] ^= 0xff;
        file["wrapped_key"]["ciphertext"] = serde_json::Value::String(B64.encode(ct));
        fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        let err = load(&path).unwrap_err().to_string();
        assert!(err.contains("could not decrypt"), "got: {err}");
        std::env::remove_var(PASSPHRASE_ENV);
    }

    #[test]
    fn session_unlocks_reads_and_writes() {
        with_passphrase(|| {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("vault.json");
            save(&path, &sample_data()).unwrap();
            start_session(&path, 7).unwrap();
            let expiry = session_expiry(&path).expect("session active");
            assert!(expiry > now_unix() + 6 * 24 * 60 * 60);

            // The session file holds the data key, never the passphrase.
            let raw = fs::read_to_string(session_path(&path)).unwrap();
            assert!(!raw.contains("test-passphrase"));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(session_path(&path))
                    .unwrap()
                    .permissions()
                    .mode();
                assert_eq!(mode & 0o777, 0o600);
            }

            // A save through the session path keeps the same wrapped key, so
            // the passphrase still opens the vault afterwards. (The true
            // "no passphrase in the environment" proof lives in the e2e
            // suite, where each invocation is a fresh process.)
            let mut data = sample_data();
            data.secrets.insert("NEW".into(), "added-later".into());
            save(&path, &data).unwrap();
            assert_eq!(load(&path).unwrap(), data);

            assert!(end_session(&path).unwrap());
            assert!(session_expiry(&path).is_none());
            assert!(!end_session(&path).unwrap());
        });
    }

    #[test]
    fn expired_session_is_deleted_and_ignored() {
        with_passphrase(|| {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("vault.json");
            save(&path, &sample_data()).unwrap();
            start_session(&path, 7).unwrap();
            // Rewind the expiry to the past.
            let session = session_path(&path);
            let mut parsed: serde_json::Value =
                serde_json::from_slice(&fs::read(&session).unwrap()).unwrap();
            parsed["expires_at"] = serde_json::Value::from(1u64);
            fs::write(&session, serde_json::to_vec(&parsed).unwrap()).unwrap();

            assert!(session_expiry(&path).is_none(), "expired session ignored");
            assert!(!session.exists(), "expired session deleted on read");
            // The vault still opens via the (env) passphrase.
            assert_eq!(load(&path).unwrap(), sample_data());
        });
    }

    #[test]
    fn stale_session_for_a_recreated_vault_is_dropped() {
        with_passphrase(|| {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("vault.json");
            save(&path, &sample_data()).unwrap();
            start_session(&path, 7).unwrap();
            // Recreate the vault: the old session key no longer fits.
            fs::remove_file(&path).unwrap();
            save(&path, &VaultData::default()).unwrap();

            assert_eq!(load(&path).unwrap(), VaultData::default());
            assert!(
                !session_path(&path).exists(),
                "mismatched session removed during unlock"
            );
        });
    }

    #[test]
    fn start_session_without_a_vault_is_a_guided_error() {
        with_passphrase(|| {
            let dir = TempDir::new().unwrap();
            let err = start_session(&dir.path().join("vault.json"), 7)
                .unwrap_err()
                .to_string();
            assert!(err.contains("no vault"), "got: {err}");
        });
    }
}
