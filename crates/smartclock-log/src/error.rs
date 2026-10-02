//! What can go wrong reading a log.

use std::path::PathBuf;

/// Why a read of the log failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file could not be opened.
    #[error("opening {} read-only: {source}", path.display())]
    Open {
        /// The file.
        path: PathBuf,
        /// What SQLite said.
        source: rusqlite::Error,
    },
    /// A query failed.
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    /// The request asked for something the log cannot answer: an
    /// unknown column, a range that ends before it starts.
    #[error("{0}")]
    Request(String),
}

/// A result whose error is a log read's.
pub type Result<T> = std::result::Result<T, Error>;
