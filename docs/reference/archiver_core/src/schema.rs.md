# `archiver_core/src/schema.rs`

[Reference index](../../README.md) · [Source](../../../../archiver_core/src/schema.rs)

Creates and checks the SQLite index. `SCHEMA_SQL` defines `scan_session`, `session_barcode`, `file_storage_volume`, and `scanned_log_entry`, with foreign keys, status checks, barcode indexes, and uniqueness for `(barcode,last_update)` and `(relative_path,volume)`. `SCHEMA_VERSION` is 2. Existing legacy indexes are rejected rather than migrated.

## Functions

### `table_has_column`

[Source, line 69](../../../../archiver_core/src/schema.rs#L69)

```rust
fn table_has_column(conn: &Connection, table: &str, column: &str) -> Result<bool>
```

Reads `PRAGMA table_info(table)` and returns whether its column-name rows contain `column`. Table names come from internal constants; SQLite preparation and decoding errors propagate.

### `apply_schema`

[Source, line 80](../../../../archiver_core/src/schema.rs#L80)

```rust
fn apply_schema(conn: &Connection) -> Result<()>
```

Reads `user_version`, detects unversioned legacy `sha256` or `chunk_start_byte` columns, and rejects legacy or unsupported versions with `SchemaMismatch`. Otherwise creates missing tables/indexes, enables foreign keys, and sets version 2. The schema statements are not wrapped in an explicit transaction here.

### `open_and_migrate`

[Source, line 111](../../../../archiver_core/src/schema.rs#L111)

```rust
pub fn open_and_migrate(path: impl AsRef<std::path::Path>) -> Result<Connection>
```

Opens or creates the supplied database file, applies the current schema checks, and returns a `Connection`. SQLite and schema incompatibility errors propagate; callers must provide any needed parent directory.

### `open_in_memory`

[Source, line 118](../../../../archiver_core/src/schema.rs#L118)

```rust
pub fn open_in_memory() -> Result<Connection>
```

Creates an in-memory database with the same schema and foreign-key configuration. Returns its connection, or the schema/SQLite error; primarily used in tests and the benchmark.
