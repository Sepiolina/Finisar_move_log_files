# `archiver_core/tests/secure_config.rs`

[Reference index](../../README.md) · [Source](../../../../archiver_core/tests/secure_config.rs)

Temporary-file tests for encrypted DB settings, key reuse, permissions, authenticated decryption, and clearing. Fixture credentials are synthetic test data.

## Functions

### `sample_config`

[Source, line 7](../../../../archiver_core/tests/secure_config.rs#L7)

```rust
fn sample_config() -> DbConfig
```

Returns a complete deterministic DB configuration for encrypted-storage tests, including timeout and retry fields.

### `save_then_load_round_trips_exactly`

[Source, line 23](../../../../archiver_core/tests/secure_config.rs#L23)

```rust
fn save_then_load_round_trips_exactly()
```

Checks every DB configuration field survives authenticated save/load unchanged.

### `load_before_any_save_returns_none_not_error`

[Source, line 44](../../../../archiver_core/tests/secure_config.rs#L44)

```rust
fn load_before_any_save_returns_none_not_error()
```

Checks a nonexistent configuration returns `Ok(None)`.

### `on_disk_config_file_does_not_contain_plaintext_password`

[Source, line 53](../../../../archiver_core/tests/secure_config.rs#L53)

```rust
fn on_disk_config_file_does_not_contain_plaintext_password()
```

Checks encrypted file bytes do not contain the fixture password, username, or database name as visible strings.

### `key_file_is_separate_from_config_file_and_owner_only_on_unix`

[Source, line 77](../../../../archiver_core/tests/secure_config.rs#L77)

```rust
fn key_file_is_separate_from_config_file_and_owner_only_on_unix()
```

Checks distinct key/config files are created and the newly created key has Unix permission bits 0600.

### `tampered_ciphertext_fails_to_decrypt_rather_than_silently_returning_wrong_data`

[Source, line 97](../../../../archiver_core/tests/secure_config.rs#L97)

```rust
fn tampered_ciphertext_fails_to_decrypt_rather_than_silently_returning_wrong_data()
```

Flips the final encrypted byte and checks authenticated decryption fails.

### `wrong_key_fails_to_decrypt`

[Source, line 115](../../../../archiver_core/tests/secure_config.rs#L115)

```rust
fn wrong_key_fails_to_decrypt()
```

Replaces the key with different 32-byte contents and checks configuration loading fails cleanly.

### `clear_removes_both_files`

[Source, line 130](../../../../archiver_core/tests/secure_config.rs#L130)

```rust
fn clear_removes_both_files()
```

Checks clearing removes both config and key and repeated clearing succeeds.

### `saving_twice_reuses_the_same_key_and_second_load_gets_latest_values`

[Source, line 147](../../../../archiver_core/tests/secure_config.rs#L147)

```rust
fn saving_twice_reuses_the_same_key_and_second_load_gets_latest_values()
```

Changes a saved password, checks key bytes stay identical, and confirms loading returns the latest value.

## Running this file

```sh
cargo test -p archiver_core --test secure_config
```
