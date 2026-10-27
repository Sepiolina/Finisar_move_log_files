use crate::db::DbConfig;
use crate::error::{ArchiveError, Result};
use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use std::path::{Path, PathBuf};

const KEY_LEN: usize = 32; // AES-256 key size
const NONCE_LEN: usize = 12; // AES-GCM standard nonce size

/// Paths for the stored encryption key and encrypted configuration file.
pub struct SecureConfigPaths {
    pub key_file: PathBuf,
    pub config_file: PathBuf,
}

impl SecureConfigPaths {
    pub fn in_dir(config_dir: &Path) -> Self {
        Self {
            key_file: config_dir.join("db_key.bin"),
            config_file: config_dir.join("db_config.enc"),
        }
    }
}

/// Loads the encryption key, generating and saving a new one if absent.
fn load_or_create_key(paths: &SecureConfigPaths) -> Result<[u8; KEY_LEN]> {
    if let Ok(bytes) = std::fs::read(&paths.key_file) {
        if bytes.len() == KEY_LEN {
            let mut key = [0u8; KEY_LEN];
            key.copy_from_slice(&bytes);
            return Ok(key);
        }
        return Err(ArchiveError::Config(format!(
            "key file at {:?} is the wrong length ({} bytes, expected {KEY_LEN}) — refusing to use it",
            paths.key_file,
            bytes.len()
        )));
    }

    let mut key = [0u8; KEY_LEN];
    OsRng.fill_bytes(&mut key);

    if let Some(parent) = paths.key_file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ArchiveError::io(parent, e))?;
    }

    write_owner_only(&paths.key_file, &key)?;
    Ok(key)
}

/// Writes contents to `path` with owner-only permissions (`0600` on Unix).
fn write_owner_only(path: &Path, contents: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| ArchiveError::io(path, e))?;
        file.write_all(contents)
            .map_err(|e| ArchiveError::io(path, e))
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents).map_err(|e| ArchiveError::io(path, e))
    }
}

/// The AES-256-GCM cipher for the stored key (creating the key if absent).
fn cipher(paths: &SecureConfigPaths) -> Result<Aes256Gcm> {
    let key_bytes = load_or_create_key(paths)?;
    Ok(Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes)))
}

/// Encrypts and saves `config` to disk.
pub fn save(paths: &SecureConfigPaths, config: &DbConfig) -> Result<()> {
    let cipher = cipher(paths)?;

    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let plaintext = serde_json::to_vec(config)
        .map_err(|e| ArchiveError::Config(format!("failed to serialize DB config: {e}")))?;
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_ref())
        .map_err(|e| ArchiveError::Config(format!("encryption failed: {e}")))?;

    // On-disk layout: [12-byte nonce][ciphertext + auth tag]
    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);

    if let Some(parent) = paths.config_file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ArchiveError::io(parent, e))?;
    }
    write_owner_only(&paths.config_file, &out)
}

/// Loads and decrypts `DbConfig`. Returns `Ok(None)` if no file exists.
pub fn load(paths: &SecureConfigPaths) -> Result<Option<DbConfig>> {
    let Ok(raw) = std::fs::read(&paths.config_file) else {
        return Ok(None);
    };
    if raw.len() < NONCE_LEN {
        return Err(ArchiveError::Config(format!(
            "config file at {:?} is too short to contain a valid nonce",
            paths.config_file
        )));
    }

    let cipher = cipher(paths)?;

    let (nonce_bytes, ciphertext) = raw.split_at(NONCE_LEN);
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher.decrypt(nonce, ciphertext).map_err(|_| {
        ArchiveError::Config(
            "failed to decrypt DB config — the key file may be missing/corrupted, or the config file was tampered with"
                .to_string(),
        )
    })?;

    let config: DbConfig = serde_json::from_slice(&plaintext)
        .map_err(|e| ArchiveError::Config(format!("decrypted config is not valid JSON: {e}")))?;
    Ok(Some(config))
}

/// Deletes both the encryption key and configuration file.
pub fn clear(paths: &SecureConfigPaths) -> Result<()> {
    remove_if_present(&paths.config_file)?;
    remove_if_present(&paths.key_file)
}

/// Removes `path`; a file that is already gone is not an error.
fn remove_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(ArchiveError::io(path, e)),
        _ => Ok(()),
    }
}
