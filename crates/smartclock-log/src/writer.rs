//! Opening a log to write it, and the version check every writer makes
//! before touching it.
//!
//! Here rather than in a daemon because more than one service writes a
//! log of this kind: the receiver daemon and the sensor service.  Each
//! keeps its own tables and its own migrations; what is here is what
//! both must do alike, including the one call that needs `unsafe`.

use std::ffi::c_int;
use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;
use rusqlite::OptionalExtension as _;
use rusqlite::ffi;
use rusqlite::params;

use crate::error::Error;
use crate::error::Result;
use crate::schema::META;

/// How long to wait for another writer before giving up on a statement.
///
/// The same five seconds rusqlite already sets on every connection it
/// opens, stated here because the writers depend on it and a default
/// is somebody else's to change.  It matters only when a second writer
/// exists -- a maintenance `sqlite3` at the prompt, a repair, an
/// operator deleting a row -- since the service is otherwise the only
/// one and WAL readers never block it.  Without the wait, such a write
/// costs the service whichever row it collided with.
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Open or create a log for writing.
///
/// WAL lets readers run against the file while the service writes.
/// NORMAL rather than FULL: a row lost to a power cut costs one
/// interval, which is not worth an fsync per row.
pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path).map_err(|source| Error::Create {
        path: path.to_owned(),
        source,
    })?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    persist_wal(&conn)?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    Ok(conn)
}

/// Keep the `-wal` and `-shm` files when the log is closed.
///
/// A reader opening a WAL log read-only needs `-shm` to exist and cannot
/// create it in a directory it may not write, which is how the viewers
/// see a service's state directory.  By default the last connection to
/// close deletes both, leaving a log no service holds open unreadable
/// to them.
fn persist_wal(conn: &Connection) -> Result<()> {
    let mut on: c_int = 1;
    #[expect(
        unsafe_code,
        reason = "rusqlite has no safe call for SQLITE_FCNTL_PERSIST_WAL"
    )]
    // SAFETY: the handle is valid while `conn` is borrowed, the schema
    // name is a NUL-terminated literal, and PERSIST_WAL reads and writes
    // one int through the pointer, which outlives the call.
    let code = unsafe {
        ffi::sqlite3_file_control(
            conn.handle(),
            c"main".as_ptr(),
            ffi::SQLITE_FCNTL_PERSIST_WAL,
            (&raw mut on).cast(),
        )
    };
    if code != ffi::SQLITE_OK {
        return Err(Error::Meta(format!(
            "keeping the log's WAL files failed with SQLite code {code}"
        )));
    }
    Ok(())
}

/// The schema version a log is stamped with, or `None` for a new one,
/// refusing a log newer than `understood`.
///
/// The metadata table is created first and alone, because the version
/// it holds decides whether the log may be touched at all: creating the
/// rest before reading it would add tables to a newer service's log and
/// then refuse it.  A newer log is refused rather than written into and
/// restamped, because a newer service may have changed what a column
/// means, and there is no undoing a bad write to the only record.
pub fn stamped_version(
    conn: &Connection,
    understood: i64,
    program: &'static str,
) -> Result<Option<i64>> {
    conn.execute_batch(META)?;
    let found: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = 'schema'", [], |row| {
            row.get(0)
        })
        .optional()?;
    let Some(found) = found else {
        return Ok(None);
    };
    let found: i64 = found
        .parse()
        .map_err(|_| Error::Meta(format!("meta.schema is {found:?}, which is not a version")))?;
    if found > understood {
        return Err(Error::NewerSchema {
            found,
            understood,
            program,
        });
    }
    Ok(Some(found))
}

/// Stamp a log with the schema it now has, and the build that wrote it:
/// a row that looks wrong is worth little without knowing what produced
/// it, and a log outlives any number of upgrades.
pub fn stamp(conn: &Connection, version: i64) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema', ?1)",
        params![version.to_string()],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES ('writer', ?1)",
        params![smartclock::VERSION],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::stamp;
    use super::stamped_version;
    use crate::error::Error;
    use crate::schema::META;
    use rusqlite::Connection;

    #[test]
    fn a_new_log_has_no_version_and_a_stamped_one_reads_back() {
        let conn = Connection::open_in_memory().expect("open");
        assert_eq!(stamped_version(&conn, 3, "test").expect("read"), None);
        stamp(&conn, 3).expect("stamp");
        assert_eq!(stamped_version(&conn, 3, "test").expect("read"), Some(3));
    }

    #[test]
    fn a_newer_log_is_refused_naming_the_program() {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch(META).expect("meta");
        stamp(&conn, 4).expect("stamp");
        let refused = stamped_version(&conn, 3, "smartclock-sensord").expect_err("newer");
        assert!(matches!(
            refused,
            Error::NewerSchema {
                found: 4,
                understood: 3,
                ..
            }
        ));
        assert!(
            refused
                .to_string()
                .contains("this smartclock-sensord understands 3")
        );
    }
}
