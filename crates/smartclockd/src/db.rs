//! The snapshot log.
//!
//! One writer, held by the daemon, and any number of readers opening
//! the same file directly.  WAL is what makes that work: a client
//! charting EFC drift reads while the daemon keeps writing, with no
//! coordination between them.
//!
//! Retention is unbounded for now, so the timestamp index is what keeps
//! the table queryable as it grows.

use std::path::Path;

use anyhow::Context as _;
use anyhow::Result;
use rusqlite::Connection;
use rusqlite::OptionalExtension as _;
use rusqlite::params;
use smartclock::snapshot::Freshness;
use smartclock::snapshot::Snapshot;

/// Bumped when the tables change shape.
const SCHEMA: i64 = 3;

/// The daemon's write connection.
#[derive(Debug)]
pub(crate) struct Log {
    conn: Connection,
}

impl Log {
    /// Open or create the log.
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("opening the log at {}", path.display()))?;
        // WAL lets readers run against the file while the daemon writes.
        // NORMAL rather than FULL: a snapshot lost to a power cut costs
        // one poll interval, which is not worth an fsync per row.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let log = Self { conn };
        log.migrate()?;
        Ok(log)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            -- One row per poll that produced a publishable snapshot.
            CREATE TABLE IF NOT EXISTS snapshot (
                id                   INTEGER PRIMARY KEY,
                at                   TEXT    NOT NULL,
                freshness            TEXT    NOT NULL,
                mode                 TEXT,
                tfom                 INTEGER,
                ffom                 INTEGER,
                time_interval_s      REAL,
                efc_percent          REAL,
                hardware_bits        INTEGER,
                holdover_waiting     TEXT,
                temperature_c        REAL,
                oven_current         REAL,
                efc_dac              INTEGER,
                holdover_active      INTEGER,
                holdover_elapsed_s   REAL,
                holdover_predicted_s REAL,
                holdover_present_s   REAL,
                tracking             INTEGER,
                not_tracking         INTEGER,
                date_raw             TEXT,
                rollover_epochs      INTEGER,
                log_count            INTEGER,
                last_error           TEXT,
                -- When each group of fields was last read.  Without
                -- these a fast row restates the ten- and sixty-second
                -- values under a fresh timestamp, and a later query
                -- cannot tell a measurement from a repeat.
                fast_at              TEXT,
                medium_at            TEXT,
                slow_at              TEXT
            );
            CREATE INDEX IF NOT EXISTS snapshot_at ON snapshot(at);

            -- The satellite table, which exists only on the status
            -- screen and so only on medium-tier polls.
            CREATE TABLE IF NOT EXISTS satellite (
                snapshot_id INTEGER NOT NULL REFERENCES snapshot(id),
                prn         INTEGER NOT NULL,
                tracked     INTEGER NOT NULL,
                acquiring   INTEGER NOT NULL,
                elevation   INTEGER,
                azimuth     INTEGER,
                signal      INTEGER
            );
            CREATE INDEX IF NOT EXISTS satellite_snapshot ON satellite(snapshot_id);

            -- Every command that was not a scheduled poll.  Phase 6
            -- fills this; the table exists now so the schema does not
            -- move once there is history worth keeping.
            CREATE TABLE IF NOT EXISTS audit (
                id      INTEGER PRIMARY KEY,
                at      TEXT NOT NULL,
                scpi    TEXT NOT NULL,
                class   TEXT,
                outcome TEXT,
                label   TEXT
            );
            CREATE INDEX IF NOT EXISTS audit_at ON audit(at);
            "#,
        )?;
        // Databases written before the per-tier columns existed keep
        // their rows; the new columns read NULL there, which says
        // honestly that the age of those fields was not recorded.
        for column in ["fast_at", "medium_at", "slow_at"] {
            if !self.has_column("snapshot", column)? {
                self.conn
                    .execute_batch(&format!("ALTER TABLE snapshot ADD COLUMN {column} TEXT"))?;
            }
        }
        // Refuse a database this binary is too old to understand
        // rather than writing into it and restamping it as ours.  A
        // newer daemon may have added columns or changed what a column
        // means, and the rows are the only record of a receiver's
        // history: there is no undoing a bad write to them.
        let found: Option<String> = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'schema'", [], |row| {
                row.get(0)
            })
            .optional()?;
        if let Some(found) = found {
            let found: i64 = found
                .parse()
                .with_context(|| format!("meta.schema is {found:?}, which is not a version"))?;
            anyhow::ensure!(
                found <= SCHEMA,
                "this database is schema {found} and this smartclockd understands {SCHEMA}; \
                 it was written by a newer version"
            );
        }
        self.conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema', ?1)",
            params![SCHEMA.to_string()],
        )?;
        Ok(())
    }

    /// Whether a table already has a column, for migrating in place.
    fn has_column(&self, table: &str, column: &str) -> Result<bool> {
        let mut statement = self.conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let mut names = statement.query_map([], |row| row.get::<_, String>(1))?;
        Ok(names.any(|name| name.is_ok_and(|n| n == column)))
    }

    /// Append a snapshot and its satellite table.
    pub(crate) fn record(&mut self, snapshot: &Snapshot) -> Result<i64> {
        let tx = self.conn.transaction()?;
        let screen = snapshot.screen.as_ref();
        tx.execute(
            "INSERT INTO snapshot (
                at, freshness, mode, tfom, ffom, time_interval_s, efc_percent,
                hardware_bits, holdover_waiting, temperature_c, oven_current, efc_dac,
                holdover_active, holdover_elapsed_s,
                holdover_predicted_s, holdover_present_s, tracking, not_tracking,
                date_raw, rollover_epochs, log_count, last_error,
                fast_at, medium_at, slow_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                       ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
            params![
                snapshot.at.to_string(),
                match snapshot.freshness {
                    Freshness::Live => "live",
                    Freshness::Stale => "stale",
                    Freshness::Disconnected => "disconnected",
                },
                snapshot.mode.map(|m| format!("{m:?}")),
                snapshot.tfom.map(|v| i64::from(v.get())),
                snapshot.ffom.map(|v| i64::from(v.get())),
                snapshot.time_interval.map(|v| v.as_secs()),
                snapshot.efc.map(|v| v.percent()),
                snapshot.hardware.map(|h| i64::from(h.bits())),
                snapshot.holdover_waiting.map(|w| format!("{w:?}")),
                snapshot.temperature,
                snapshot.oven_current,
                snapshot.efc_dac,
                snapshot.holdover_duration.map(|h| h.active),
                snapshot.holdover_duration.map(|h| h.elapsed.as_secs()),
                snapshot.holdover_predicted.map(|v| v.as_secs()),
                snapshot.holdover_present.map(|v| v.as_secs()),
                screen.and_then(|s| s.tracking),
                screen.and_then(|s| s.not_tracking),
                snapshot.date.map(|d| d.raw().to_string()),
                snapshot.date.and_then(|d| d.rollover()).map(|r| r.epochs),
                snapshot.log_count,
                snapshot.polled.any_error(),
                snapshot.polled.fast.at.map(|t| t.to_string()),
                snapshot.polled.medium.at.map(|t| t.to_string()),
                snapshot.polled.slow.at.map(|t| t.to_string()),
            ],
        )?;
        let id = tx.last_insert_rowid();

        if let Some(screen) = screen {
            let mut insert = tx.prepare(
                "INSERT INTO satellite (snapshot_id, prn, tracked, acquiring,
                                        elevation, azimuth, signal)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for sat in &screen.satellites {
                insert.execute(params![
                    id,
                    i64::from(sat.prn.get()),
                    sat.tracked,
                    sat.acquiring,
                    sat.elevation.map(|d| i64::from(d.get())),
                    sat.azimuth.map(|d| i64::from(d.get())),
                    sat.signal.map(|s| i64::from(s.raw())),
                ])?;
            }
        }
        tx.commit()?;
        Ok(id)
    }

    /// Record a command that was not a scheduled poll.
    ///
    /// Kept in the same database as the telemetry on purpose.  "What
    /// did I do to it, and what did EFC do afterwards" is one query
    /// when they share a file and a reconciliation when they do not.
    ///
    /// Socket permissions are the whole of the authorization here, so
    /// the daemon cannot tell two clients apart: this records what was
    /// done, not by whom.
    pub(crate) fn audit(
        &mut self,
        scpi: &str,
        class: &str,
        outcome: &str,
        label: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO audit (at, scpi, class, outcome, label) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                jiff::Timestamp::now().to_string(),
                scpi,
                class,
                outcome,
                label
            ],
        )?;
        Ok(())
    }

    /// How many commands have been recorded.
    pub(crate) fn audit_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM audit", [], |r| r.get(0))?)
    }

    /// How many snapshots are held.
    pub(crate) fn count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM snapshot", [], |r| r.get(0))?)
    }
}

#[cfg(test)]
mod tests {
    use super::Log;
    use super::SCHEMA;
    use rusqlite::Connection;

    /// A database file of our own, under the test runner's temp dir.
    fn scratch(name: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("smartclockd-{name}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn a_database_from_a_newer_daemon_is_refused_not_restamped() {
        // The stamp was written with INSERT OR REPLACE and never read,
        // so a database written by a later schema was silently relabelled
        // as this one's and written into.  Snapshots are the only record
        // of a receiver's history; there is no undoing that.
        let path = scratch("newer");
        let conn = Connection::open(&path).expect("create the database");
        conn.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO meta VALUES ('schema', '99');",
        )
        .expect("stamp it as newer");
        drop(conn);

        let refused = Log::open(&path).expect_err("a newer schema must be refused");
        let why = format!("{refused:#}");
        assert!(
            why.contains("99"),
            "the message should name the version: {why}"
        );

        // And the stamp is left alone rather than overwritten with ours.
        let conn = Connection::open(&path).expect("reopen");
        let found: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'schema'", [], |row| {
                row.get(0)
            })
            .expect("the stamp survives");
        assert_eq!(found, "99");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_database_of_our_own_schema_opens_and_is_stamped() {
        let path = scratch("ours");
        let log = Log::open(&path).expect("a fresh database opens");
        let found: String = log
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'schema'", [], |row| {
                row.get(0)
            })
            .expect("a stamp");
        assert_eq!(found, SCHEMA.to_string());
        drop(log);
        // Reopening its own database is not a refusal.
        Log::open(&path).expect("reopening our own schema");
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }
}
