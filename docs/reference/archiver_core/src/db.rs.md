# `archiver_core/src/db.rs`

[Reference index](../../README.md) · [Source](../../../../archiver_core/src/db.rs)

Fetches archival candidates from SQL Server and computes UTC date windows. `ArchivalCandidate` carries a barcode and raw last-update string. `DbConfig` holds connection credentials, TLS switches, request timeout, retry count, and base delay; JSON duration fields use milliseconds.

## Functions

### `serialize`

[Source, line 39](../../../../archiver_core/src/db.rs#L39)

```rust
pub fn serialize<S: Serializer>(d: &Duration, s: S) -> std::result::Result<S::Ok, S::Error>
```

Serializes a `Duration` as an unsigned millisecond count through the supplied Serde serializer. Returns the serializer result; the `as_millis()` value is cast to `u64`.

### `deserialize`

[Source, line 42](../../../../archiver_core/src/db.rs#L42)

```rust
pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Duration, D::Error>
```

Reads an unsigned millisecond count from Serde and returns `Duration::from_millis`. Type/range errors are Serde errors.

### `default`

[Source, line 48](../../../../archiver_core/src/db.rs#L48)

```rust
fn default() -> Self
```

Returns empty connection strings, port 1433, encryption disabled, certificate trust enabled, 30-second timeout, three retries, and a two-second base delay. Does not open a connection.

### `from_env`

[Source, line 66](../../../../archiver_core/src/db.rs#L66)

```rust
pub fn from_env() -> Result<Self>
```

Builds `DbConfig` from required `DB_SERVER`, `DB_NAME`, `DB_USER`, and `DB_PASSWORD`; optional variables configure port, TLS, timeout, retries, and delay. Missing required variables return `Config`; malformed numeric options fall back to defaults.

### `req`

[Source, line 67](../../../../archiver_core/src/db.rs#L67)

```rust
fn req(key: &str) -> Result<String>
```

Nested helper of `from_env`: reads one required environment variable, returning its string or a configuration error naming the key.

### `opt_bool`

[Source, line 72](../../../../archiver_core/src/db.rs#L72)

```rust
fn opt_bool(key: &str, default: bool) -> bool
```

Nested helper of `from_env`: returns the supplied default when unset; otherwise only case-insensitive `true` or `1` mean true. All other present strings mean false.

### `opt`

[Source, line 79](../../../../archiver_core/src/db.rs#L79)

```rust
fn opt<T: FromStr>(key: &str, default: T) -> T
```

Nested generic helper of `from_env`: parses one environment value with `FromStr`, using the supplied default for missing or unparsable values.

### `to_tiberius_config`

[Source, line 101](../../../../archiver_core/src/db.rs#L101)

```rust
fn to_tiberius_config(&self) -> Config
```

Maps host, port, database, SQL authentication, and TLS switches to a Tiberius configuration. Disabling encryption selects `NotSupported`; this helper does not connect or apply timeouts.

### `build_query`

[Source, line 118](../../../../archiver_core/src/db.rs#L118)

```rust
pub fn build_query(from_date: Option<&str>, to_date: &str) -> String
```

Returns the fixed shipment serial/trace join with `DISTINCT` and `NOLOCK`. `from_date`, when present, is inclusive and `to_date` is exclusive. Date strings are interpolated directly, so the caller must supply trusted date values; no validation or parameter binding occurs here.

### `query_archival_candidates`

[Source, line 136](../../../../archiver_core/src/db.rs#L136)

```rust
pub async fn query_archival_candidates(
    config: &DbConfig,
    from_date: Option<&str>,
    to_date: &str,
) -> Result<Vec<ArchivalCandidate>>
```

Builds the query and awaits one initial attempt plus up to `retry_attempts` retries. Every failure is retried using `retry_base_delay * 2^attempt`; returns candidate rows on success or the last error after exhaustion.

### `io_timeout_err`

[Source, line 159](../../../../archiver_core/src/db.rs#L159)

```rust
fn io_timeout_err(message: &str) -> ArchiveError
```

Constructs a SQL Server I/O error with `TimedOut` and the supplied message. Used to normalize Tokio timeout failures.

### `io_connect_err`

[Source, line 166](../../../../archiver_core/src/db.rs#L166)

```rust
fn io_connect_err(e: std::io::Error) -> ArchiveError
```

Converts a standard I/O error into the SQL Server error variant while retaining its kind and message.

### `try_query_once`

[Source, line 174](../../../../archiver_core/src/db.rs#L174)

```rust
async fn try_query_once(config: &DbConfig, sql: &str) -> Result<Vec<ArchivalCandidate>>
```

Connects TCP with a request timeout, attempts `TCP_NODELAY`, performs the Tiberius handshake, starts the query with a separate request timeout, and collects its first result set. Missing/null barcode and timestamp values become empty strings. The handshake and subsequent result collection are outside those explicit timeout wrappers.

### `compute_resume_window`

[Source, line 219](../../../../archiver_core/src/db.rs#L219)

```rust
pub fn compute_resume_window(
    last_successful_scan_to_date: Option<&str>,
    lookback_days: i64,
) -> Result<(Option<String>, String)>
```

Returns `(optional_from,to)` as date strings: `to` is the current UTC day minus `lookback_days`; `from` is the previous successful query end minus one day. No previous end gives no lower bound. Parsing failures propagate from `ymd_to_days`.

### `days_since_epoch_now`

[Source, line 234](../../../../archiver_core/src/db.rs#L234)

```rust
fn days_since_epoch_now() -> i64
```

Returns whole UTC days since the Unix epoch. A system time before the epoch falls back to zero before division by 86,400.

### `days_to_ymd`

[Source, line 247](../../../../archiver_core/src/db.rs#L247)

```rust
fn days_to_ymd(days: i64) -> String
```

Converts an epoch-relative day number to a Gregorian `YYYY-MM-DD` string using civil-calendar arithmetic. Has no side effects.

### `ymd_to_days`

[Source, line 262](../../../../archiver_core/src/db.rs#L262)

```rust
fn ymd_to_days(s: &str) -> Result<i64>
```

Splits a date into three hyphen-separated numeric fields and converts them to epoch-relative days. Returns `Config` for malformed structure or unparsable numbers; it is not a complete calendar-range validator.
