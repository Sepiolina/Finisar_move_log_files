# `ui/src/settings.rs`

[Reference index](../../README.md) · [Source](../../../../ui/src/settings.rs)

Plain JSON UI preferences. `UiSettings` stores optional source/output/staging/index paths, extension filter, cached scan end, dry-run flag, and email settings. Credentials are stored separately by `secure_config`; email fields have Serde defaults for older settings files.

## Functions

### `default`

[Source, line 33](../../../../ui/src/settings.rs#L33)

```rust
fn default() -> Self
```

Returns unset paths and scan cache, `txt`/`zip` extensions, dry run disabled, and email disabled with empty relay/recipient strings.

### `config_dir`

[Source, line 50](../../../../ui/src/settings.rs#L50)

```rust
pub fn config_dir() -> Option<PathBuf>
```

Uses `ProjectDirs` with qualifier `com` and application/organization `log-archiver` to return the OS-specific user config directory, or `None` when unavailable.

### `settings_path`

[Source, line 55](../../../../ui/src/settings.rs#L55)

```rust
pub fn settings_path() -> PathBuf
```

Returns `settings.json` in the user config directory, falling back to a relative `settings.json` in the process working directory.

### `load`

[Source, line 61](../../../../ui/src/settings.rs#L61)

```rust
pub fn load() -> Self
```

Loads from the default settings path using `load_from`.

### `load_from`

[Source, line 65](../../../../ui/src/settings.rs#L65)

```rust
pub fn load_from(path: &Path) -> Self
```

Reads and deserializes settings from a supplied file. Any read or JSON failure silently returns all default settings; missing non-defaulted fields can therefore reset the entire loaded value.

### `save`

[Source, line 72](../../../../ui/src/settings.rs#L72)

```rust
pub fn save(&self)
```

Delegates persistence to `save_to` at the default settings path. Returns no success/error value.

### `save_to`

[Source, line 76](../../../../ui/src/settings.rs#L76)

```rust
pub fn save_to(&self, path: &Path)
```

Attempts to create the parent directory, pretty-serialize preferences, and write the file. All directory, serialization, and write failures are silently ignored; saving is not an atomic replacement.

### `email_settings_round_trip`

[Source, line 94](../../../../ui/src/settings.rs#L94)

```rust
fn email_settings_round_trip()
```

Unit test checking enabled email, relay URL, and recipient survive save/load.

### `loads_pre_existing_settings_file_missing_email_fields`

[Source, line 116](../../../../ui/src/settings.rs#L116)

```rust
fn loads_pre_existing_settings_file_missing_email_fields()
```

Unit test confirming older JSON without email fields retains its existing source path and receives default email values, while an obsolete extra field is ignored.
