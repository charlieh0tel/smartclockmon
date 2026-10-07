//! What can go wrong reading or writing a log.

use std::path::PathBuf;

/// Why a read or a write of the log failed.
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
    /// The file could not be opened for writing.
    #[error("opening {} to write it: {source}", path.display())]
    Create {
        /// The file.
        path: PathBuf,
        /// What SQLite said.
        source: rusqlite::Error,
    },
    /// The log was written by a newer program than this one, which
    /// refuses it rather than write into it.  No retry fixes it.
    #[error(
        "this log is schema {found} and this {program} understands {understood}; \
         it was written by a newer version"
    )]
    NewerSchema {
        /// The schema the log is stamped with.
        found: i64,
        /// The newest schema this program understands.
        understood: i64,
        /// Which program refused it.
        program: &'static str,
    },
    /// The log's own record of itself is not what it should be.
    #[error("{0}")]
    Meta(String),
    /// A query failed.
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    /// The request asked for something the log cannot answer: an
    /// unknown column, a range that ends before it starts.
    #[error("{0}")]
    Request(String),
}

/// A result whose error is a log's.
pub type Result<T> = std::result::Result<T, Error>;
