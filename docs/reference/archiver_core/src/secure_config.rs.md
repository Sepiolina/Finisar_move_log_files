# `archiver_core/src/secure_config.rs`

[Reference index](../../README.md) · [Source](../../../../archiver_core/src/secure_config.rs)

Stores SQL Server settings using AES-256-GCM. `SecureConfigPaths` identifies a separate 32-byte key and encrypted configuration; the encrypted layout is a 12-byte nonce followed by ciphertext and authentication tag. The local key file is required to decrypt saved settings.

## Functions

### `in_dir`

[Source, line 18](../../../../archiver_core/src/secure_config.rs#L18)

```rust
pub fn in_dir(config_dir: &Path) -> Self
```

Returns paths `<config_dir>/db_key.bin` and `<config_dir>/db_config.enc`. Does not create the directory or either file.

### `load_or_create_key`

[Source, line 27](../../../../archiver_core/src/secure_config.rs#L27)

```rust
fn load_or_create_key(paths: &SecureConfigPaths) -> Result<[u8; KEY_LEN]>
```

Reads and returns an existing 32-byte key, rejecting readable keys of any other length. If the key cannot be read, generates random bytes, creates parent directories, and writes a key; any read error enters this creation path, not just absence.

### `write_owner_only`

[Source, line 53](../../../../archiver_core/src/secure_config.rs#L53)

```rust
fn write_owner_only(path: &Path, contents: &[u8]) -> Result<()>
```

Creates or truncates the target and writes the supplied bytes. On Unix, mode 0600 is applied at creation; an existing file retains its existing permissions. Other platforms use `fs::write`. Writes are not atomic replacements and are not explicitly synced.

### `cipher`

[Source, line 75](../../../../archiver_core/src/secure_config.rs#L75)

```rust
fn cipher(paths: &SecureConfigPaths) -> Result<Aes256Gcm>
```

Loads or creates the key and returns an initialized AES-256-GCM cipher. Key validation and filesystem failures propagate.

### `save`

[Source, line 81](../../../../archiver_core/src/secure_config.rs#L81)

```rust
pub fn save(paths: &SecureConfigPaths, config: &DbConfig) -> Result<()>
```

Serializes `DbConfig` as JSON, generates a fresh random nonce, encrypts it, creates the config parent directory, and writes nonce plus ciphertext/tag. Returns configuration errors for serialization/encryption or path-bearing I/O errors for writes.

### `load`

[Source, line 106](../../../../archiver_core/src/secure_config.rs#L106)

```rust
pub fn load(paths: &SecureConfigPaths) -> Result<Option<DbConfig>>
```

Reads the encrypted config and returns `Some(DbConfig)` after authenticated decryption and JSON decoding. Any config read failure returns `Ok(None)`; short nonce data, authentication failure, or invalid decrypted JSON returns `Config`. Loading a readable config may create a missing key through `cipher`, then fail authentication.

### `clear`

[Source, line 135](../../../../archiver_core/src/secure_config.rs#L135)

```rust
pub fn clear(paths: &SecureConfigPaths) -> Result<()>
```

Deletes the config first and then the key, returning the first failure. Missing files are tolerated; deletion is not transactional.

### `remove_if_present`

[Source, line 141](../../../../archiver_core/src/secure_config.rs#L141)

```rust
fn remove_if_present(path: &Path) -> Result<()>
```

Checks existence and removes the file if present, returning a path-bearing I/O error on failure. The existence check and removal are separate operations.
