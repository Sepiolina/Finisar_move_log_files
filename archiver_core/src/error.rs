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

    /// An error reported by `mochi-core`. `code` is MOCHI's stable error
    /// name (`code.as_str()`, e.g. `LOCK_CONFLICT`); branch on it, never on
    /// the message.
    #[error("MOCHI error {code}: {message}")]
    Mochi { code: String, message: String },

    /// The archive's publication lock is held by another writer
    /// (`LOCK_CONFLICT`). Nothing was written; retry later.
    #[error("archive {path:?} is locked by another writer (LOCK_CONFLICT): {message}")]
    ArchiveLocked { path: PathBuf, message: String },

    /// An earlier write to this archive was interrupted and left bytes after
    /// the last commit (`UNCOMMITTED_TAIL`). We never truncate automatically:
    /// a person must run `mochi verify` and then `mochi append --truncate-tail`.
    #[error(
        "archive {path:?} has an uncommitted tail from an interrupted write (UNCOMMITTED_TAIL): {message}. \
         Nothing was changed and staging was kept. Run `mochi verify`, then `mochi append <archive> --truncate-tail`, then re-run."
    )]
    UncommittedTail { path: PathBuf, message: String },

    /// Post-commit verification failed; staged files were NOT deleted.
    #[error("archive verification failed for {volume}: {reason}")]
    VerificationFailed { volume: String, reason: String },

    /// The database was created by an incompatible (older) schema.
    #[error("index database has an incompatible schema (found version {found}, need {expected}): {hint}")]
    SchemaMismatch { found: i64, expected: i64, hint: String },

    /// An archive entry name is not safe to turn into a path on disk.
    #[error("unsafe archive path rejected: {0:?}")]
    UnsafePath(String),

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

impl From<mochi_core::MochiError> for ArchiveError {
    fn from(e: mochi_core::MochiError) -> Self {
        ArchiveError::Mochi {
            code: e.code.as_str().to_string(),
            message: e.message,
        }
    }
}

impl ArchiveError {
    /// Maps a MOCHI error for the archive at `path`, giving `LOCK_CONFLICT`
    /// and `UNCOMMITTED_TAIL` their own clear variants.
    pub fn from_mochi(path: &std::path::Path, e: mochi_core::MochiError) -> Self {
        use mochi_core::ErrorCode;
        match e.code {
            ErrorCode::LockConflict => ArchiveError::ArchiveLocked {
                path: path.to_path_buf(),
                message: e.message,
            },
            ErrorCode::UncommittedTail => ArchiveError::UncommittedTail {
                path: path.to_path_buf(),
                message: e.message,
            },
            _ => ArchiveError::from(e),
        }
    }

    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        ArchiveError::Io {
            path: Some(path.into()),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, ArchiveError>;
