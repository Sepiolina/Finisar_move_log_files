//! SQL Server query module for fetching old barcode records for archival.

use crate::error::{ArchiveError, Result};
use std::time::Duration;
use tiberius::{AuthMethod, Client, Config};
use tokio::net::TcpStream;
use tokio_util::compat::TokioAsyncWriteCompatExt;

/// Represents a single barcode record fetched for archiving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivalCandidate {
    pub barcode: String,
    pub last_update: String, // Kept as raw string to avoid external date library dependencies
}

/// Holds SQL Server database connection settings.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DbConfig {
    pub server: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: String,
    pub encrypt: bool,
    pub trust_server_certificate: bool,
    #[serde(with = "duration_millis")]
    pub request_timeout: Duration,
    pub retry_attempts: u32,
    #[serde(with = "duration_millis")]
    pub retry_base_delay: Duration,
}

/// Converts Rust `Duration` values to/from simple milliseconds (u64) for JSON serialization.
mod duration_millis {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_millis() as u64)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Duration, D::Error> {
        Ok(Duration::from_millis(u64::deserialize(d)?))
    }
}

impl Default for DbConfig {
    fn default() -> Self {
        Self {
            server: String::new(),
            port: 1433,
            database: String::new(),
            username: String::new(),
            password: String::new(),
            encrypt: false,
            trust_server_certificate: true,
            request_timeout: Duration::from_millis(30_000),
            retry_attempts: 3,
            retry_base_delay: Duration::from_millis(2_000),
        }
    }
}

impl DbConfig {
    /// Loads settings from environment variables (e.g. `DB_SERVER`, `DB_PASSWORD`), using defaults for optional ones.
    pub fn from_env() -> Result<Self> {
        fn req(key: &str) -> Result<String> {
            std::env::var(key).map_err(|_| ArchiveError::Config(format!("missing required environment variable {key}")))
        }
        fn opt_bool(key: &str, default: bool) -> bool {
            std::env::var(key)
                .ok()
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
                .unwrap_or(default)
        }
        fn opt_u64(key: &str, default: u64) -> u64 {
            std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
        }
        fn opt_u32(key: &str, default: u32) -> u32 {
            std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
        }
        fn opt_u16(key: &str, default: u16) -> u16 {
            std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
        }

        Ok(Self {
            server: req("DB_SERVER")?,
            port: opt_u16("DB_PORT", 1433),
            database: req("DB_NAME")?,
            username: req("DB_USER")?,
            password: req("DB_PASSWORD")?,
            encrypt: opt_bool("DB_ENCRYPT", false),
            trust_server_certificate: opt_bool("DB_TRUST_SERVER_CERTIFICATE", true),
            request_timeout: Duration::from_millis(opt_u64("DB_REQUEST_TIMEOUT_MS", 30_000)),
            retry_attempts: opt_u32("DB_RETRY_ATTEMPTS", 3),
            retry_base_delay: Duration::from_millis(opt_u64("DB_RETRY_BASE_DELAY_MS", 2_000)),
        })
    }

    /// Translates these settings into Tiberius SQL driver configuration.
    fn to_tiberius_config(&self) -> Config {
        let mut config = Config::new();
        config.host(&self.server);
        config.port(self.port);
        config.database(&self.database);
        config.authentication(AuthMethod::sql_server(&self.username, &self.password));
        if self.trust_server_certificate {
            config.trust_cert();
        }
        if !self.encrypt {
            config.encryption(tiberius::EncryptionLevel::NotSupported);
        }
        config
    }
}

/// Builds the T-SQL query string filtered by the specified date range.
pub fn build_query(from_date: Option<&str>, to_date: &str) -> String {
    let date_clause = match from_date {
        Some(from) => format!("sft.last_update >= '{from}' AND sft.last_update < '{to_date}'"),
        None => format!("sft.last_update < '{to_date}'"),
    };

    format!(
        "SELECT DISTINCT \
            sfs.serial_no AS BARCODE, \
            CONVERT(VARCHAR(50), sft.last_update, 120) AS last_update \
        FROM ForthUtilities.dbo.ShipmentFinisarSerial sfs WITH (NOLOCK) \
        JOIN ForthUtilities.dbo.ShipmentFinisarTrace sft WITH (NOLOCK) \
            ON sfs.trace_id = sft.trace_id \
        WHERE {date_clause};"
    )
}

/// Fetches records matching the target date range. Retries automatically on failure using exponential backoff.
pub async fn query_archival_candidates(
    config: &DbConfig,
    from_date: Option<&str>,
    to_date: &str,
) -> Result<Vec<ArchivalCandidate>> {
    let sql = build_query(from_date, to_date);
    let mut last_err = None;

    for attempt in 0..=config.retry_attempts {
        match try_query_once(config, &sql).await {
            Ok(rows) => return Ok(rows),
            Err(e) => {
                last_err = Some(e);
                if attempt < config.retry_attempts {
                    // Exponential backoff delay (e.g. 2s, 4s, 8s...)
                    let delay = config.retry_base_delay * 2u32.pow(attempt);
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    Err(last_err.expect("loop runs at least once, so an error is always set on the failure path"))
}

fn io_timeout_err(message: &str) -> ArchiveError {
    ArchiveError::SqlServer(tiberius::error::Error::Io {
        kind: std::io::ErrorKind::TimedOut,
        message: message.to_string(),
    })
}

fn io_connect_err(e: std::io::Error) -> ArchiveError {
    ArchiveError::SqlServer(tiberius::error::Error::Io {
        kind: e.kind(),
        message: e.to_string(),
    })
}

/// Attempts a single connection and query execution against SQL Server.
async fn try_query_once(config: &DbConfig, sql: &str) -> Result<Vec<ArchivalCandidate>> {
    let tiberius_config = config.to_tiberius_config();

    let addr = tiberius_config
        .get_addr()
        .to_string();

    // Establish TCP connection with timeout
    let tcp = tokio::time::timeout(config.request_timeout, TcpStream::connect(&addr))
        .await
        .map_err(|_| io_timeout_err("connection attempt timed out"))?
        .map_err(io_connect_err)?;
    tcp.set_nodelay(true).ok();

    // Initialize SQL client session
    let mut client = Client::connect(tiberius_config, tcp.compat_write())
        .await
        .map_err(ArchiveError::from)?;

    // Execute SQL query with timeout
    let stream = tokio::time::timeout(config.request_timeout, client.query(sql, &[]))
        .await
        .map_err(|_| io_timeout_err("query timed out"))?
        .map_err(ArchiveError::from)?;

    let rows = stream.into_first_result().await.map_err(ArchiveError::from)?;

    let mut results = Vec::with_capacity(rows.len());
    for row in rows {
        let barcode: &str = row.get("BARCODE").unwrap_or_default();
        let last_update = row
            .get::<&str, _>("last_update")
            .map(str::to_string)
            .unwrap_or_default();
        results.push(ArchivalCandidate {
            barcode: barcode.to_string(),
            last_update,
        });
    }
    Ok(results)
}

/// Calculates `(from_date, to_date)` strings (YYYY-MM-DD) for the scan window.
/// Uses custom date math functions to avoid pulling in an extra date library.
pub fn compute_resume_window(
    last_successful_scan_to_date: Option<&str>,
    lookback_days: i64,
) -> Result<(Option<String>, String)> {
    use std::time::{SystemTime, UNIX_EPOCH};

    // Low-level date math helper functions
    fn days_since_epoch_now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
            / 86_400
    }
    fn days_to_ymd(days: i64) -> String {
        let z = days + 719468;
        let era = if z >= 0 { z } else { z - 146096 } / 146097;
        let doe = (z - era * 146097) as u64;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe as i64 + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        format!("{y:04}-{m:02}-{d:02}")
    }
    fn ymd_to_days(s: &str) -> Result<i64> {
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() != 3 {
            return Err(ArchiveError::Config(format!("invalid date string {s:?}, expected YYYY-MM-DD")));
        }
        let y: i64 = parts[0].parse().map_err(|_| ArchiveError::Config(format!("bad year in {s:?}")))?;
        let m: i64 = parts[1].parse().map_err(|_| ArchiveError::Config(format!("bad month in {s:?}")))?;
        let d: i64 = parts[2].parse().map_err(|_| ArchiveError::Config(format!("bad day in {s:?}")))?;
        let y2 = if m <= 2 { y - 1 } else { y };
        let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
        let yoe = (y2 - era * 400) as u64;
        let mp = ((m + 9) % 12) as u64;
        let doy = (153 * mp + 2) / 5 + d as u64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        Ok(era * 146097 + doe as i64 - 719468)
    }

    let to_days = days_since_epoch_now() - lookback_days;
    let to_date = days_to_ymd(to_days);

    let from_date = match last_successful_scan_to_date {
        Some(prev_to) => {
            let prev_days = ymd_to_days(prev_to)?;
            Some(days_to_ymd(prev_days - 1))
        }
        None => None,
    };

    Ok((from_date, to_date))
}