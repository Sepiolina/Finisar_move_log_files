//! Tests for `secure_config`: round-trip save/load, tamper detection,
//! and confirming the on-disk file is not plaintext-readable.

use archiver_core::db::DbConfig;
use archiver_core::secure_config::{self, SecureConfigPaths};

fn sample_config() -> DbConfig {
    DbConfig {
        server: "192.168.1.1".to_string(),
        port: 1433,
        database: "ForthUtilities".to_string(),
        username: "dbusername".to_string(),
        password: "correct-horse-battery-staple".to_string(),
        encrypt: false,
        trust_server_certificate: true,
        request_timeout: std::time::Duration::from_millis(30_000),
        retry_attempts: 3,
        retry_base_delay: std::time::Duration::from_millis(2_000),
    }
}

#[test]
fn save_then_load_round_trips_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let paths = SecureConfigPaths::in_dir(dir.path());

    let config = sample_config();
    secure_config::save(&paths, &config).unwrap();

    let loaded = secure_config::load(&paths).unwrap().expect("config should load after being saved");
    assert_eq!(loaded.server, config.server);
    assert_eq!(loaded.port, config.port);
    assert_eq!(loaded.database, config.database);
    assert_eq!(loaded.username, config.username);
    assert_eq!(loaded.password, config.password);
    assert_eq!(loaded.encrypt, config.encrypt);
    assert_eq!(loaded.trust_server_certificate, config.trust_server_certificate);
    assert_eq!(loaded.request_timeout, config.request_timeout);
    assert_eq!(loaded.retry_attempts, config.retry_attempts);
    assert_eq!(loaded.retry_base_delay, config.retry_base_delay);
}

#[test]
fn load_before_any_save_returns_none_not_error() {
    let dir = tempfile::tempdir().unwrap();
    let paths = SecureConfigPaths::in_dir(dir.path());

    let result = secure_config::load(&paths).unwrap();
    assert!(result.is_none(), "no saved config yet should be Ok(None), not an error");
}

#[test]
fn on_disk_config_file_does_not_contain_plaintext_password() {
    let dir = tempfile::tempdir().unwrap();
    let paths = SecureConfigPaths::in_dir(dir.path());

    let config = sample_config();
    secure_config::save(&paths, &config).unwrap();

    let raw_bytes = std::fs::read(&paths.config_file).unwrap();
    let raw_as_lossy_string = String::from_utf8_lossy(&raw_bytes);
    assert!(
        !raw_as_lossy_string.contains(&config.password),
        "the plaintext password must not appear anywhere in the encrypted file on disk"
    );
    assert!(
        !raw_as_lossy_string.contains(&config.username),
        "the plaintext username must not appear anywhere in the encrypted file on disk"
    );
    assert!(
        !raw_as_lossy_string.contains("ForthUtilities"),
        "the database name must not appear in plaintext in the encrypted file on disk"
    );
}

#[test]
fn key_file_is_separate_from_config_file_and_owner_only_on_unix() {
    let dir = tempfile::tempdir().unwrap();
    let paths = SecureConfigPaths::in_dir(dir.path());

    secure_config::save(&paths, &sample_config()).unwrap();

    assert!(paths.key_file.exists());
    assert!(paths.config_file.exists());
    assert_ne!(paths.key_file, paths.config_file);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&paths.key_file).unwrap().permissions().mode();
        // Mask to the permission bits only (mode() includes file-type bits too).
        assert_eq!(mode & 0o777, 0o600, "key file should be owner-read/write only");
    }
}

#[test]
fn tampered_ciphertext_fails_to_decrypt_rather_than_silently_returning_wrong_data() {
    let dir = tempfile::tempdir().unwrap();
    let paths = SecureConfigPaths::in_dir(dir.path());

    secure_config::save(&paths, &sample_config()).unwrap();

    // Flip a byte in the ciphertext (well past the 12-byte nonce prefix)
    // to simulate corruption or tampering.
    let mut raw = std::fs::read(&paths.config_file).unwrap();
    let tamper_index = raw.len() - 1;
    raw[tamper_index] ^= 0xFF;
    std::fs::write(&paths.config_file, &raw).unwrap();

    let result = secure_config::load(&paths);
    assert!(result.is_err(), "tampered ciphertext must fail to decrypt (AEAD authentication should catch this), not return garbage data");
}

#[test]
fn wrong_key_fails_to_decrypt() {
    let dir = tempfile::tempdir().unwrap();
    let paths = SecureConfigPaths::in_dir(dir.path());

    secure_config::save(&paths, &sample_config()).unwrap();

    // Overwrite the key with different random bytes, simulating e.g. a
    // corrupted or replaced key file.
    std::fs::write(&paths.key_file, [0x42u8; 32]).unwrap();

    let result = secure_config::load(&paths);
    assert!(result.is_err(), "decrypting with the wrong key must fail cleanly, not silently succeed with garbage");
}

#[test]
fn clear_removes_both_files() {
    let dir = tempfile::tempdir().unwrap();
    let paths = SecureConfigPaths::in_dir(dir.path());

    secure_config::save(&paths, &sample_config()).unwrap();
    assert!(paths.config_file.exists());
    assert!(paths.key_file.exists());

    secure_config::clear(&paths).unwrap();
    assert!(!paths.config_file.exists());
    assert!(!paths.key_file.exists());

    // Clearing an already-empty store should not error.
    secure_config::clear(&paths).unwrap();
}

#[test]
fn saving_twice_reuses_the_same_key_and_second_load_gets_latest_values() {
    let dir = tempfile::tempdir().unwrap();
    let paths = SecureConfigPaths::in_dir(dir.path());

    let mut config = sample_config();
    secure_config::save(&paths, &config).unwrap();
    let key_after_first_save = std::fs::read(&paths.key_file).unwrap();

    config.password = "a-different-password".to_string();
    secure_config::save(&paths, &config).unwrap();
    let key_after_second_save = std::fs::read(&paths.key_file).unwrap();

    assert_eq!(key_after_first_save, key_after_second_save, "the key must not change on subsequent saves");

    let loaded = secure_config::load(&paths).unwrap().unwrap();
    assert_eq!(loaded.password, "a-different-password");
}
