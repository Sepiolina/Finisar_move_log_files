use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ArchiveError {
    #[error("I/O error at {path:?}: {source}")]
    Io {
        path: Option<PathBuf>,
        #[source]
        source: std::io::Error,
    },

    #[error("tar build error: {0}")]
    Tar(#[source] std::io::Error),

    #[error("zstd error: {0}")]
    Zstd(#[source] std::io::Error),

    #[error("chunk verification failed for chunk starting at byte {chunk_start_byte}: {reason}")]
    VerificationFailed {
        chunk_start_byte: u64,
        reason: String,
    },

    #[error("no active chunk is open — call start_chunk() first")]
    NoActiveChunk,

    #[error("database error: {0}")]
    Db(#[source] rusqlite::Error),

    #[error("SQL Server error: {0}")]
    SqlServer(#[source] tiberius::error::Error),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("duplicate scan: barcode {barcode:?} with last_update {last_update:?} was already recorded in a previous session")]
    DuplicateBarcodeScan { barcode: String, last_update: String },

    #[error("no matching log entry found for {0:?}")]
    EntryNotFound(String),

    #[error("statistics email error: {0}")]
    Email(String),
}

impl From<rusqlite::Error> for ArchiveError {
    fn from(e: rusqlite::Error) -> Self {
        ArchiveError::Db(e)
    }
}

impl From<tiberius::error::Error> for ArchiveError {
    fn from(e: tiberius::error::Error) -> Self {
        ArchiveError::SqlServer(e)
    }
}

impl ArchiveError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        ArchiveError::Io {
            path: Some(path.into()),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, ArchiveError>;
