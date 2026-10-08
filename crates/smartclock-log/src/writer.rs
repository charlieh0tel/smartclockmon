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
use crate::timestamp;

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

/// Rebuild each table `definitions` creates in the shape it gives,
/// converting the timestamp columns `times` names from the text schema
/// 11 and earlier wrote to integer nanoseconds, and stamp the log
/// `version`.
///
/// A column's type cannot be altered in place, so each table is made
/// anew under another name, filled, and renamed over the old one, as
/// SQLite documents for any change it cannot `ALTER`; its indexes are
/// made again after.  One transaction, the stamp in it, so a log is
/// converted whole or not at all however the process ends.  Every
/// timestamp is checked before any is converted: one in another form
/// refuses the conversion rather than being rounded, guessed at or
/// dropped.
///
/// The file is vacuumed after, since the rebuild leaves the old
/// tables' pages free in it.
///
/// Foreign keys are not enforced while the tables are swapped, since
/// dropping a table other rows point at would count as deleting them,
/// and are checked whole before the commit instead.  The setting has
/// no effect inside a transaction, so it is changed around it.
pub fn convert_to_integer_times(
    conn: &mut Connection,
    definitions: &str,
    times: &[(&str, &[&str])],
    version: i64,
) -> Result<()> {
    let enforced: bool = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    conn.pragma_update(None, "foreign_keys", false)?;
    let converted = (|| {
        let tx = conn.transaction()?;
        rebuild_with_integer_times(&tx, definitions, times)?;
        let broken: i64 =
            tx.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })?;
        if broken > 0 {
            return Err(Error::Meta(format!(
                "{broken} rows would point at rows that do not exist; nothing was converted"
            )));
        }
        stamp(&tx, version)?;
        tx.commit()?;
        Ok(())
    })();
    conn.pragma_update(None, "foreign_keys", enforced)?;
    converted?;
    // The old tables' pages are free but still in the file, which
    // would otherwise keep the size it had plus the new tables'.  Once
    // only: a failure here leaves a converted log, merely larger.
    conn.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")?;
    Ok(())
}

/// The rebuild [`convert_to_integer_times`] makes, inside its
/// transaction.
fn rebuild_with_integer_times(
    conn: &Connection,
    definitions: &str,
    times: &[(&str, &[&str])],
) -> Result<()> {
    let target = Connection::open_in_memory()?;
    target.execute_batch(definitions)?;
    let shapes: Vec<(String, String, String, String)> = target
        .prepare(
            "SELECT type, name, tbl_name, sql FROM sqlite_master
             WHERE sql IS NOT NULL ORDER BY rowid",
        )?
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let columns_of = |conn: &Connection, table: &str| -> Result<Vec<String>> {
        Ok(conn
            .prepare(&format!("PRAGMA table_info({table})"))?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<_>>()?)
    };
    let tables: Vec<&(String, String, String, String)> =
        shapes.iter().filter(|(kind, ..)| kind == "table").collect();
    let mut rebuilt = Vec::new();
    for (_, table, _, _) in &tables {
        let held = columns_of(conn, table)?;
        if held.is_empty() {
            continue;
        }
        let converted = times
            .iter()
            .find(|(name, _)| name == table)
            .map(|(_, columns)| *columns)
            .ok_or_else(|| Error::Meta(format!("no timestamp columns are listed for {table}")))?;
        for column in converted.iter().filter(|c| held.iter().any(|h| h == *c)) {
            let unconvertible: i64 = conn.query_row(
                &format!(
                    "SELECT count(*) FROM {table} WHERE {column} IS NOT NULL AND NOT {}",
                    timestamp::convertible(column)
                ),
                [],
                |row| row.get(0),
            )?;
            if unconvertible > 0 {
                return Err(Error::Meta(format!(
                    "{table}.{column} holds {unconvertible} timestamps not in the form \
                     schema 11 wrote; nothing was converted"
                )));
            }
        }
        // A column the definitions no longer have is dropped by the
        // rebuild, which is right only for one that holds nothing.
        let wanted = columns_of(&target, table)?;
        for column in held.iter().filter(|h| !wanted.contains(h)) {
            let values: i64 =
                conn.query_row(&format!("SELECT count({column}) FROM {table}"), [], |row| {
                    row.get(0)
                })?;
            if values > 0 {
                return Err(Error::Meta(format!(
                    "{table}.{column} is no longer a column and holds {values} values; \
                     nothing was converted"
                )));
            }
        }
        rebuilt.push((table.as_str(), held, converted));
    }
    for (table, held, converted) in &rebuilt {
        let (.., sql) = tables
            .iter()
            .find(|(_, name, ..)| name == table)
            .ok_or_else(|| Error::Meta(format!("{table} has no definition")))?;
        let body = sql
            .find('(')
            .map(|open| &sql[open..])
            .ok_or_else(|| Error::Meta(format!("{table}'s definition has no columns")))?;
        let fresh = format!("new_{table}");
        conn.execute_batch(&format!("CREATE TABLE {fresh} {body}"))?;
        let kept: Vec<&String> = columns_of(&target, table)?
            .iter()
            .filter_map(|c| held.iter().find(|h| *h == c))
            .collect();
        let names = kept
            .iter()
            .map(|c| c.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let values = kept
            .iter()
            .map(|c| {
                if converted.contains(&c.as_str()) {
                    timestamp::from_text(c)
                } else {
                    (*c).clone()
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        conn.execute_batch(&format!(
            "INSERT INTO {fresh} ({names}) SELECT {values} FROM {table};
             DROP TABLE {table};
             ALTER TABLE {fresh} RENAME TO {table};"
        ))?;
    }
    for (_, _, table, sql) in shapes.iter().filter(|(kind, ..)| kind == "index") {
        if rebuilt.iter().any(|(t, ..)| t == table) {
            conn.execute_batch(sql)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::convert_to_integer_times;
    use super::stamp;
    use super::stamped_version;
    use crate::error::Error;
    use crate::schema::META;
    use crate::timestamp::Stored;
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

    /// Three tables as schema 11 would hold them: text timestamps, a
    /// row pointing at another, an index on a time.
    const OLD: &str = "
        CREATE TABLE receiver (id INTEGER PRIMARY KEY, serial TEXT NOT NULL,
                               first_seen TEXT NOT NULL);
        CREATE TABLE snapshot (id INTEGER PRIMARY KEY, at TEXT NOT NULL,
                               fast_at TEXT, value REAL,
                               receiver_id INTEGER REFERENCES receiver(id));
        CREATE INDEX snapshot_at ON snapshot(at);
        CREATE TABLE note (id INTEGER PRIMARY KEY, at TEXT NOT NULL, text TEXT NOT NULL);
        INSERT INTO receiver VALUES (1, 'A', '2026-09-01T00:00:00.000000000Z');
        INSERT INTO snapshot VALUES
            (1, '2026-09-01T00:00:01.000000001Z', '2026-09-01T00:00:01.000000001Z', 1.5, 1),
            (2, '2026-09-01T00:00:02.123456789Z', NULL, NULL, 1);
        INSERT INTO note VALUES (1, '2026-09-01T00:00:03.500000000Z', 'swapped the stick');";

    /// The same tables as the conversion is to leave them.
    const NEW: &str = "
        CREATE TABLE IF NOT EXISTS receiver (id INTEGER PRIMARY KEY, serial TEXT NOT NULL,
                                             first_seen INTEGER NOT NULL) STRICT;
        CREATE TABLE IF NOT EXISTS snapshot (id INTEGER PRIMARY KEY, at INTEGER NOT NULL,
                                             fast_at INTEGER, value REAL,
                                             receiver_id INTEGER REFERENCES receiver(id)) STRICT;
        CREATE INDEX IF NOT EXISTS snapshot_at ON snapshot(at);
        CREATE TABLE IF NOT EXISTS note (id INTEGER PRIMARY KEY, at INTEGER NOT NULL,
                                         text TEXT NOT NULL) STRICT;";

    const TIMES: [(&str, &[&str]); 3] = [
        ("receiver", &["first_seen"]),
        ("snapshot", &["at", "fast_at"]),
        ("note", &["at"]),
    ];

    /// A schema 11 log, as `OLD` builds it with `extra` run after.
    fn old_log(extra: &str) -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch(META).expect("meta");
        stamp(&conn, 11).expect("stamp");
        conn.execute_batch(OLD).expect("the old tables");
        conn.execute_batch(extra).expect("the extra rows");
        conn
    }

    /// Every table's rows, as text, to tell whether anything changed.
    fn contents(conn: &Connection) -> Vec<String> {
        ["receiver", "snapshot", "note"]
            .iter()
            .flat_map(|table| {
                let mut statement = conn
                    .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                    .expect("prepare");
                let width = statement.column_count();
                statement
                    .query_map([], |row| {
                        (0..width)
                            .map(|i| {
                                row.get::<_, rusqlite::types::Value>(i)
                                    .map(|v| format!("{v:?}"))
                            })
                            .collect::<rusqlite::Result<Vec<_>>>()
                            .map(|v| v.join("|"))
                    })
                    .expect("query")
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .expect("rows")
            })
            .collect()
    }

    fn version(conn: &Connection) -> String {
        conn.query_row("SELECT value FROM meta WHERE key = 'schema'", [], |row| {
            row.get(0)
        })
        .expect("the stamp")
    }

    #[test]
    fn a_log_is_converted_whole_to_the_nanosecond() {
        let mut conn = old_log("");
        let enforced: bool = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("pragma");
        convert_to_integer_times(&mut conn, NEW, &TIMES, 12).expect("convert");
        assert_eq!(version(&conn), "12");
        let at = |s: &str| Stored(s.parse().expect("a time"));
        let rows: Vec<(Stored, Option<Stored>, Option<f64>, i64)> = conn
            .prepare("SELECT at, fast_at, value, receiver_id FROM snapshot ORDER BY id")
            .expect("prepare")
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .expect("query")
            .collect::<rusqlite::Result<_>>()
            .expect("rows");
        assert_eq!(
            rows,
            vec![
                (
                    at("2026-09-01T00:00:01.000000001Z"),
                    Some(at("2026-09-01T00:00:01.000000001Z")),
                    Some(1.5),
                    1
                ),
                (at("2026-09-01T00:00:02.123456789Z"), None, None, 1),
            ]
        );
        let note: Stored = conn
            .query_row("SELECT at FROM note", [], |row| row.get(0))
            .expect("the note");
        assert_eq!(note, at("2026-09-01T00:00:03.5Z"));
        let strict: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND sql LIKE '%STRICT'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(strict, 3);
        let indexed: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = 'snapshot_at'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(indexed, 1);
        let after: bool = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("pragma");
        assert_eq!(after, enforced, "foreign keys are enforced as before");
    }

    #[test]
    fn a_timestamp_in_another_form_refuses_the_conversion_and_changes_nothing() {
        let mut conn = old_log("INSERT INTO note VALUES (2, '2026-09-01', 'a date alone');");
        let before = contents(&conn);
        let refused = convert_to_integer_times(&mut conn, NEW, &TIMES, 12).expect_err("refused");
        assert!(
            refused.to_string().contains("note.at holds 1 timestamps"),
            "{refused}"
        );
        assert_eq!(version(&conn), "11");
        assert_eq!(contents(&conn), before);
    }

    #[test]
    fn a_failure_after_the_tables_are_rebuilt_leaves_the_log_as_it_was() {
        // A row pointing at a receiver that does not exist is found by
        // the check before the commit, after every table is rebuilt:
        // the same position a power cut late in the conversion leaves.
        let mut conn = old_log(
            "PRAGMA foreign_keys = OFF;
             INSERT INTO snapshot VALUES (3, '2026-09-01T00:00:04.000000000Z', NULL, NULL, 99);",
        );
        conn.pragma_update(None, "foreign_keys", true)
            .expect("pragma");
        let before = contents(&conn);
        let refused = convert_to_integer_times(&mut conn, NEW, &TIMES, 12).expect_err("refused");
        assert!(
            refused.to_string().contains("would point at rows"),
            "{refused}"
        );
        assert_eq!(version(&conn), "11");
        assert_eq!(contents(&conn), before);
    }

    /// Every table `definitions` makes is listed in `times`, and every
    /// column listed is an integer column of its table.
    fn times_cover(definitions: &str, times: &[(&str, &[&str])]) {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch(definitions).expect("the tables");
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<rusqlite::Result<_>>()
            .expect("tables");
        for table in &tables {
            let listed = times.iter().find(|(name, _)| name == table);
            assert!(listed.is_some(), "{table} is not listed");
        }
        for (table, columns) in times {
            assert!(tables.iter().any(|t| t == table), "{table} is not a table");
            for column in *columns {
                let kind: String = conn
                    .query_row(
                        &format!("SELECT type FROM pragma_table_info('{table}') WHERE name = ?1"),
                        [column],
                        |row| row.get(0),
                    )
                    .unwrap_or_else(|_| panic!("{table}.{column} is not a column"));
                assert_eq!(kind, "INTEGER", "{table}.{column}");
            }
        }
    }

    #[test]
    fn every_timestamp_column_of_both_logs_is_listed_for_conversion() {
        times_cover(crate::schema::TABLES, &crate::schema::TIMES);
        times_cover(crate::sensors::TABLES, &crate::sensors::TIMES);
    }

    #[test]
    fn a_column_no_longer_defined_is_dropped_only_if_it_holds_nothing() {
        let empty = "ALTER TABLE note ADD COLUMN label TEXT;";
        let mut conn = old_log(empty);
        convert_to_integer_times(&mut conn, NEW, &TIMES, 12).expect("an empty column goes");
        let mut conn = old_log(&format!("{empty} UPDATE note SET label = 'kept';"));
        let before = contents(&conn);
        let refused = convert_to_integer_times(&mut conn, NEW, &TIMES, 12).expect_err("refused");
        assert!(
            refused
                .to_string()
                .contains("note.label is no longer a column"),
            "{refused}"
        );
        assert_eq!(contents(&conn), before);
    }
}
