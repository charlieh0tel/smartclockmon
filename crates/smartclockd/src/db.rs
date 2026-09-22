//! The snapshot log.
//!
//! One writer, held by the daemon, and any number of readers opening
//! the same file directly.  WAL is what makes that work: a client
//! charting EFC drift reads while the daemon keeps writing, with no
//! coordination between them.
//!
//! Retention is unbounded for now, so the timestamp index is what keeps
//! the table queryable as it grows.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;

use anyhow::Context as _;
use anyhow::Result;
use rusqlite::Connection;
use rusqlite::OptionalExtension as _;
use rusqlite::params;
use smartclock::snapshot::Freshness;
use smartclock::snapshot::Snapshot;

/// Bumped when the tables change shape.
const SCHEMA: i64 = 7;

/// The daemon's write connection.
#[derive(Debug)]
pub(crate) struct Log {
    conn: Connection,
    /// The row in `receiver` that everything written now belongs to.
    ///
    /// `None` until a receiver has answered `*IDN?`, which is a real
    /// state and not an oversight: the daemon publishes snapshots from
    /// the moment it attaches, and a row written before the identity
    /// was known is honestly of an unknown unit.
    current: Option<i64>,
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
        let mut log = Self {
            conn,
            current: None,
        };
        log.migrate()?;
        Ok(log)
    }

    fn migrate(&mut self) -> Result<()> {
        // The metadata table first and alone, because the version it
        // holds decides whether this database may be touched at all.
        // Creating the rest before reading it meant a database from a
        // newer daemon had three tables and seven columns added to it
        // and was then refused, which is the opposite of what the
        // refusal is for: a future schema that renamed one of these
        // would find it silently resurrected.
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )?;
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

        self.conn.execute_batch(
            r#"
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
                oven_tempco          REAL,
                efc_dac              INTEGER,
                alarm_bits           INTEGER,
                operation_bits       INTEGER,
                holdover_bits        INTEGER,
                powerup_bits         INTEGER,
                holdover_active      INTEGER,
                holdover_elapsed_s   REAL,
                holdover_predicted_s REAL,
                holdover_present_s   REAL,
                tracking             INTEGER,
                not_tracking         INTEGER,
                date_raw             TEXT,
                time_utc             TEXT,
                rollover_epochs      INTEGER,
                log_count            INTEGER,
                last_error           TEXT,
                -- When each group of fields was last read.  Without
                -- these a fast row restates the ten- and sixty-second
                -- values under a fresh timestamp, and a later query
                -- cannot tell a measurement from a repeat.
                fast_at              TEXT,
                medium_at            TEXT,
                slow_at              TEXT,
                -- Which unit this row describes.  NULL in rows written
                -- before the log recorded that at all.
                receiver_id          INTEGER REFERENCES receiver(id)
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

            -- Every receiver that has written to this log.
            --
            -- A log file outlives the daemon and the receiver alike,
            -- and a bench where units are swapped accumulates more than
            -- one unit's history in one file.  Without this the rows
            -- are anonymous: you can see that two receivers wrote and
            -- not which rows belong to which, which makes every
            -- long-run comparison worthless.
            CREATE TABLE IF NOT EXISTS receiver (
                id           INTEGER PRIMARY KEY,
                -- The serial alone identifies the unit.  Firmware and
                -- even the reported model can change under it; the
                -- serial is what stays.
                serial       TEXT NOT NULL UNIQUE,
                manufacturer TEXT,
                model        TEXT,
                -- As last seen, since an upgrade changes it.
                firmware     TEXT,
                first_seen   TEXT NOT NULL,
                last_seen    TEXT NOT NULL
            );

            -- Every change in the receiver's alarm condition register.
            --
            -- The snapshot table carries the same register sampled
            -- every poll, which answers "what was it at 14:02" but
            -- makes "when did it change" a scan.  These rows are the
            -- changes alone, which is what anyone reading a history
            -- actually wants and what the monitor and the browser show.
            --
            -- The register itself is read with *STB?, which is real
            -- time and non-destructive.  The event registers behind it
            -- are deliberately never read: reading one clears it, which
            -- clears the alarm, which puts out a lamp that belongs to
            -- whoever is standing at the instrument.
            CREATE TABLE IF NOT EXISTS receiver_event (
                id      INTEGER PRIMARY KEY,
                at      TEXT    NOT NULL,
                -- Which register, as the short names used in the code:
                -- operation, questionable, hardware, holdover, powerup.
                register TEXT   NOT NULL,
                bits    INTEGER NOT NULL,
                -- The bits named, so a row can be read without the
                -- manual and without this daemon's bit tables.
                decoded TEXT    NOT NULL,
                receiver_id INTEGER REFERENCES receiver(id)
            );
            CREATE INDEX IF NOT EXISTS receiver_event_at ON receiver_event(at);

            -- The transition filters in force when those events were
            -- captured.
            --
            -- Kept because the events cannot be read without them.  A
            -- filter selects which condition transitions latch, and
            -- this receiver ships with every negative filter at zero:
            -- faults latch appearing and never clearing.  Without this
            -- table, the absence of a clear-event looks like evidence a
            -- fault persisted, when it only means nobody enabled the
            -- transition that would have recorded its end.
            CREATE TABLE IF NOT EXISTS receiver_filter (
                receiver_id INTEGER NOT NULL REFERENCES receiver(id),
                register    TEXT    NOT NULL,
                -- Which transitions latch: positive, negative.
                positive    INTEGER,
                negative    INTEGER,
                at          TEXT    NOT NULL,
                PRIMARY KEY (receiver_id, register)
            );

            -- Entries taken from the receiver's error queue.  Reading
            -- an entry removes it from the receiver, so once the queue
            -- has been drained this table is the only copy there is.
            CREATE TABLE IF NOT EXISTS receiver_error (
                id      INTEGER PRIMARY KEY,
                -- When it was read, which is not when it happened: the
                -- queue carries no timestamps of its own.
                at      TEXT    NOT NULL,
                code    INTEGER NOT NULL,
                message TEXT    NOT NULL,
                receiver_id INTEGER REFERENCES receiver(id)
            );
            CREATE INDEX IF NOT EXISTS receiver_error_at ON receiver_error(at);

            -- The receiver's own diagnostic log, copied out entry by
            -- entry.  Its ring holds 222 entries and then stops
            -- recording, so anything not copied out is eventually lost.
            CREATE TABLE IF NOT EXISTS receiver_log (
                id      INTEGER PRIMARY KEY,
                -- When we read it.
                at      TEXT    NOT NULL,
                -- The receiver's own entry number, which restarts at 1
                -- when the log is cleared.
                entry   INTEGER NOT NULL,
                -- The entry's own timestamp, kept as written.  It comes
                -- from the receiver's calendar and carries whatever GPS
                -- week rollover that calendar has; folding it in here
                -- would destroy the evidence of the rollover.
                stamp   TEXT,
                message TEXT    NOT NULL,
                receiver_id INTEGER REFERENCES receiver(id),
                -- Which run of the log's numbering this entry belongs
                -- to.  Clearing the log restarts the numbering at one,
                -- so the entry number alone orders a generation and
                -- says nothing across one; without this there is no
                -- ordering over the stored columns that is right both
                -- within a generation and across a clear.  The stamp
                -- cannot stand in: before the first GPS lock it is
                -- elapsed time since boot on a stale date.
                generation INTEGER NOT NULL DEFAULT 0,
                -- Within one unit and one generation an entry number
                -- is the receiver's own key, and re-reading an entry
                -- must not duplicate it.
                UNIQUE (receiver_id, generation, entry)
            );

            -- Every command a client asked for.  Not a complete record
            -- of what was sent: the poll schedule is not audited, and
            -- neither are the journal's own queries, which are polls in
            -- everything but the tier they run on.
            CREATE TABLE IF NOT EXISTS audit (
                id      INTEGER PRIMARY KEY,
                at      TEXT NOT NULL,
                scpi    TEXT NOT NULL,
                class   TEXT,
                outcome TEXT,
                label   TEXT,
                receiver_id INTEGER REFERENCES receiver(id)
            );
            CREATE INDEX IF NOT EXISTS audit_at ON audit(at);
            "#,
        )?;
        // Databases written before the per-tier columns existed keep
        // their rows; the new columns read NULL there, which says
        // honestly that the age of those fields was not recorded.
        for (column, kind) in [
            ("fast_at", "TEXT"),
            ("medium_at", "TEXT"),
            ("slow_at", "TEXT"),
            ("oven_tempco", "REAL"),
            ("alarm_bits", "INTEGER"),
            ("operation_bits", "INTEGER"),
            ("holdover_bits", "INTEGER"),
            ("powerup_bits", "INTEGER"),
            ("time_utc", "TEXT"),
            // Rows written before this existed stay NULL: the log does
            // not know which unit produced them, and guessing would put
            // one receiver's history under another's name.
            ("receiver_id", "INTEGER REFERENCES receiver(id)"),
        ] {
            if !self.has_column("snapshot", column)? {
                self.conn
                    .execute_batch(&format!("ALTER TABLE snapshot ADD COLUMN {column} {kind}"))?;
            }
        }
        for table in ["audit", "receiver_error", "receiver_log"] {
            if !self.has_column(table, "receiver_id")? {
                self.conn.execute_batch(&format!(
                    "ALTER TABLE {table} ADD COLUMN receiver_id INTEGER REFERENCES receiver(id)"
                ))?;
            }
        }
        self.adopt_log_generations()?;
        self.conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema', ?1)",
            params![SCHEMA.to_string()],
        )?;
        // Which build last wrote here.  A row that looks wrong is worth
        // little without knowing what produced it, and the database
        // outlives any number of upgrades.
        self.conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('writer', ?1)",
            params![smartclock::VERSION],
        )?;
        Ok(())
    }

    /// Whether a table already has a column, for migrating in place.
    /// Give a pre-schema-7 `receiver_log` its generation column.
    ///
    /// The column cannot simply be added: the old table's uniqueness
    /// was over the entry's content, and the new one is over
    /// `(receiver, generation, entry)`, which SQLite will not alter in
    /// place.  So the table is rebuilt.
    ///
    /// Existing rows have their generation inferred.  Within one
    /// generation the receiver numbers each entry once, so walking a
    /// receiver's rows in the order they were read and starting a new
    /// generation whenever an entry number comes round again recovers
    /// the boundaries.  It is not guesswork about times: a repeated
    /// entry number is what a clear leaves behind.
    fn adopt_log_generations(&mut self) -> Result<()> {
        if self.has_column("receiver_log", "generation")? {
            return Ok(());
        }
        let rows: Vec<(i64, Option<i64>, i64)> = self
            .conn
            .prepare(
                "SELECT id, receiver_id, entry FROM receiver_log
                 ORDER BY receiver_id, at, id",
            )?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<std::result::Result<_, _>>()?;
        let mut generation = HashMap::new();
        let mut seen: HashMap<Option<i64>, HashSet<i64>> = HashMap::new();
        let mut current: HashMap<Option<i64>, i64> = HashMap::new();
        for (id, receiver, entry) in rows {
            let entries = seen.entry(receiver).or_default();
            if !entries.insert(entry) {
                entries.clear();
                entries.insert(entry);
                *current.entry(receiver).or_default() += 1;
            }
            generation.insert(id, current.get(&receiver).copied().unwrap_or(0));
        }
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            r#"
            CREATE TABLE receiver_log_new (
                id      INTEGER PRIMARY KEY,
                at      TEXT    NOT NULL,
                entry   INTEGER NOT NULL,
                stamp   TEXT,
                message TEXT    NOT NULL,
                receiver_id INTEGER REFERENCES receiver(id),
                generation INTEGER NOT NULL DEFAULT 0,
                UNIQUE (receiver_id, generation, entry)
            );
            "#,
        )?;
        {
            let mut insert = tx.prepare(
                "INSERT OR IGNORE INTO receiver_log_new
                     (id, at, entry, stamp, message, receiver_id, generation)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            let mut read =
                tx.prepare("SELECT id, at, entry, stamp, message, receiver_id FROM receiver_log")?;
            let mut found = read.query([])?;
            while let Some(row) = found.next()? {
                let id: i64 = row.get(0)?;
                insert.execute(params![
                    id,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    generation.get(&id).copied().unwrap_or(0),
                ])?;
            }
        }
        tx.execute_batch(
            "DROP TABLE receiver_log;
             ALTER TABLE receiver_log_new RENAME TO receiver_log;",
        )?;
        tx.commit()?;
        Ok(())
    }

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
                hardware_bits, holdover_waiting, temperature_c, oven_current, oven_tempco,
                efc_dac, alarm_bits, operation_bits, holdover_bits, powerup_bits,
                holdover_active, holdover_elapsed_s,
                holdover_predicted_s, holdover_present_s, tracking, not_tracking,
                date_raw, time_utc, rollover_epochs, log_count, last_error,
                fast_at, medium_at, slow_at, receiver_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                       ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29,
                       ?30, ?31, ?32)",
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
                snapshot.oven_tempco,
                snapshot.efc_dac,
                snapshot.alarm.map(|r| i64::from(r.bits())),
                snapshot.operation.map(|r| i64::from(r.bits())),
                snapshot.holdover_state.map(|r| i64::from(r.bits())),
                snapshot.powerup.map(|r| i64::from(r.bits())),
                snapshot.holdover_duration.map(|h| h.active),
                snapshot.holdover_duration.map(|h| h.elapsed.as_secs()),
                snapshot.holdover_predicted.map(|v| v.as_secs()),
                snapshot.holdover_present.map(|v| v.as_secs()),
                screen.and_then(|s| s.tracking),
                screen.and_then(|s| s.not_tracking),
                snapshot.date.map(|d| d.raw().to_string()),
                snapshot.time.map(|t| t.to_string()),
                snapshot.date.and_then(|d| d.rollover()).map(|r| r.epochs),
                snapshot.log_count,
                snapshot.polled.any_error(),
                snapshot.polled.fast.at.map(|t| t.to_string()),
                snapshot.polled.medium.at.map(|t| t.to_string()),
                snapshot.polled.slow.at.map(|t| t.to_string()),
                self.current,
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
            "INSERT INTO audit (at, scpi, class, outcome, label, receiver_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                jiff::Timestamp::now().to_string(),
                scpi,
                class,
                outcome,
                label,
                self.current,
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

    /// Note which receiver is attached, so that what follows is
    /// written against it.
    ///
    /// A log file outlives the daemon that wrote it and says nothing
    /// about its subject otherwise: the identity was printed to the
    /// journal at startup and kept nowhere that travels with the data.
    /// Worse, a bench where units are swapped puts two receivers'
    /// history in one file with no way to tell the rows apart.
    ///
    /// Keyed on the serial alone, because that is the only field that
    /// identifies the unit: firmware changes under it, and comparing
    /// the whole of `*IDN?` would call an upgrade a different receiver.
    ///
    /// Returns the serials of any other units that have written here,
    /// which is worth saying out loud the first time it happens.
    pub(crate) fn note_receiver(&mut self, identity: &str) -> Result<Vec<String>> {
        // The joined form is exactly what the receiver answered, so the
        // library's own parser reads it rather than a second splitter
        // here that could disagree with it.  An identity that will not
        // parse is still an identity: it keys on the whole string
        // rather than being dropped.
        let parsed = smartclock::parse::identity(identity).ok();
        let field = |get: fn(&smartclock::parse::Identity) -> &str| {
            parsed.as_ref().map(|id| get(id).to_owned())
        };
        let serial = parsed
            .as_ref()
            .map_or(identity, |id| id.serial.as_str())
            .to_owned();
        let now = jiff::Timestamp::now().to_string();
        self.conn.execute(
            "INSERT INTO receiver (serial, manufacturer, model, firmware, first_seen, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT(serial) DO UPDATE SET
                 -- Firmware is as last seen, so an upgrade is recorded
                 -- rather than leaving the row describing a build that
                 -- is no longer on the unit.
                 firmware = excluded.firmware,
                 last_seen = excluded.last_seen",
            params![
                serial,
                field(|id| &id.manufacturer),
                field(|id| &id.model),
                field(|id| &id.firmware),
                now,
            ],
        )?;
        self.current = Some(self.conn.query_row(
            "SELECT id FROM receiver WHERE serial = ?1",
            params![serial],
            |row| row.get(0),
        )?);
        let mut others = self
            .conn
            .prepare("SELECT serial FROM receiver WHERE serial <> ?1 ORDER BY first_seen")?;
        let others: Vec<String> = others
            .query_map(params![serial], |row| row.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(others)
    }

    /// Every receiver that has written here, newest attachment last.
    pub(crate) fn receivers(&self) -> Result<Vec<(String, String)>> {
        let mut statement = self
            .conn
            .prepare("SELECT serial, model FROM receiver ORDER BY first_seen")?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                ))
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }

    /// Record one entry taken from the receiver's error queue.
    pub(crate) fn record_error(&mut self, code: i32, message: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO receiver_error (at, code, message, receiver_id)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                jiff::Timestamp::now().to_string(),
                code,
                message,
                self.current,
            ],
        )?;
        Ok(())
    }

    /// The alarm as last recorded for one receiver, if ever.
    ///
    /// Read at startup so a restart does not write a row saying the
    /// alarm is what it already was.  The comparison the daemon makes
    /// is against what is written down, not against what this process
    /// happens to remember, and those differ every time it restarts.
    pub(crate) fn last_alarm(&self, receiver: i64) -> Result<Option<u16>> {
        let bits: Option<i64> = self
            .conn
            .query_row(
                "SELECT bits FROM receiver_event
                 WHERE receiver_id = ?1 AND register = 'alarm'
                 ORDER BY id DESC LIMIT 1",
                params![receiver],
                |row| row.get(0),
            )
            .optional()?;
        Ok(bits.and_then(|b| u16::try_from(b).ok()))
    }

    /// Record a change in the receiver's alarm condition.
    pub(crate) fn record_event(&mut self, register: &str, bits: u16, decoded: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO receiver_event (at, register, bits, decoded, receiver_id)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                jiff::Timestamp::now().to_string(),
                register,
                i64::from(bits),
                decoded,
                self.current,
            ],
        )?;
        Ok(())
    }

    /// Record the transition filters a receiver is running, so the
    /// events captured beside them can be interpreted.
    ///
    /// Replaces rather than appends: these are configuration, not
    /// history, and a receiver whose filters changed will show the
    /// change in `at`.
    pub(crate) fn record_filter(
        &mut self,
        register: &str,
        positive: Option<i64>,
        negative: Option<i64>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO receiver_filter
                 (receiver_id, register, positive, negative, at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                self.current,
                register,
                positive,
                negative,
                jiff::Timestamp::now().to_string(),
            ],
        )?;
        Ok(())
    }

    /// Record one of the receiver's diagnostic log entries.
    ///
    /// Entries are re-read whenever their number is in doubt, so this
    /// ignores one already held and reports whether anything was
    /// written.
    pub(crate) fn record_log_entry(
        &mut self,
        generation: i64,
        entry: i64,
        stamp: Option<&str>,
        message: &str,
    ) -> Result<bool> {
        let written = self.conn.execute(
            "INSERT OR IGNORE INTO receiver_log
                 (at, entry, stamp, message, receiver_id, generation)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                jiff::Timestamp::now().to_string(),
                entry,
                stamp,
                message,
                self.current,
                generation,
            ],
        )?;
        Ok(written > 0)
    }

    /// The newest generation of one receiver's log that we hold.
    ///
    /// Zero when nothing is held, so a first pass and a log never
    /// cleared agree.
    pub(crate) fn log_generation(&self, receiver: i64) -> Result<i64> {
        Ok(self
            .conn
            .query_row(
                "SELECT MAX(generation) FROM receiver_log WHERE receiver_id = ?1",
                params![receiver],
                |row| row.get::<_, Option<i64>>(0),
            )?
            .unwrap_or(0))
    }

    /// The span of diagnostic log entry numbers already held for one
    /// receiver, lowest and highest.
    ///
    /// Lets a restarted daemon resume a backfill instead of walking the
    /// whole log again: at sixteen entries a minute a full log takes a
    /// quarter of an hour, so a daemon restarted more often than that
    /// re-read the newest entries for ever and never reached the
    /// oldest.
    ///
    /// The span is the extremes, not proof that everything between them
    /// is held: an entry the receiver would not give up leaves a gap
    /// that only a full re-walk fills.  Those are logged when they
    /// happen.
    pub(crate) fn log_span(&self, receiver: i64, generation: i64) -> Result<Option<(i64, i64)>> {
        Ok(self.conn.query_row(
            "SELECT MIN(entry), MAX(entry) FROM receiver_log
             WHERE receiver_id = ?1 AND generation = ?2",
            params![receiver, generation],
            |row| {
                Ok(row
                    .get::<_, Option<i64>>(0)?
                    .zip(row.get::<_, Option<i64>>(1)?))
            },
        )?)
    }

    /// Whether every entry from 1 to `count` is held for one receiver.
    ///
    /// The precondition for erasing the receiver's copy.  Counting rows
    /// is not enough: a duplicate could make the total right while a
    /// gap hides inside it, so this checks that the distinct entry
    /// numbers run from one to `count` without interruption.
    pub(crate) fn log_complete(&self, receiver: i64, generation: i64, count: i64) -> Result<bool> {
        let (rows, lowest, highest): (i64, Option<i64>, Option<i64>) = self.conn.query_row(
            "SELECT COUNT(DISTINCT entry), MIN(entry), MAX(entry)
             FROM receiver_log WHERE receiver_id = ?1 AND generation = ?2",
            params![receiver, generation],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        Ok(rows == count && lowest == Some(1) && highest == Some(count))
    }

    /// Which receiver rows written now belong to.
    pub(crate) fn current_receiver(&self) -> Option<i64> {
        self.current
    }

    /// How many error queue entries, diagnostic log entries and
    /// register events are held.
    pub(crate) fn journal_counts(&self) -> Result<(i64, i64, i64)> {
        let count = |table: &str| -> Result<i64> {
            Ok(self
                .conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?)
        };
        Ok((
            count("receiver_error")?,
            count("receiver_log")?,
            count("receiver_event")?,
        ))
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
    fn an_older_database_gains_the_columns_it_is_missing() {
        // The alternative to migrating in place is refusing the file,
        // and the file is the only copy of the receiver's history.
        let path = scratch("older");
        let conn = Connection::open(&path).expect("create the database");
        conn.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO meta VALUES ('schema', '3');
             CREATE TABLE snapshot (id INTEGER PRIMARY KEY, at TEXT NOT NULL);",
        )
        .expect("write an older schema");
        drop(conn);

        let log = Log::open(&path).expect("an older database should open");
        for column in [
            "oven_tempco",
            "operation_bits",
            "holdover_bits",
            "powerup_bits",
        ] {
            assert!(
                log.has_column("snapshot", column)
                    .expect("read the columns"),
                "{column} should have been added"
            );
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_old_log_is_split_into_generations_by_its_repeated_numbers() {
        // Schema 6 had no generation column, so a database that lived
        // across a clear holds two runs of entry numbers in one table
        // with nothing to tell them apart.  The boundary is recoverable
        // without guessing at times: within a generation the receiver
        // issues each entry number once, so a number coming round again
        // is where the clear was.
        //
        // The rows are written in the order the backfill reads them --
        // newest down to oldest, then the new log upwards from one --
        // because that is the order the migration has to cope with.
        let path = scratch("generations");
        let old = Connection::open(&path).expect("make an old database");
        old.execute_batch(
            r#"
            CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            INSERT INTO meta VALUES ('schema', '6');
            CREATE TABLE receiver (
                id INTEGER PRIMARY KEY, serial TEXT UNIQUE NOT NULL,
                manufacturer TEXT, model TEXT, firmware TEXT,
                first_seen TEXT, last_seen TEXT
            );
            INSERT INTO receiver (id, serial) VALUES (1, 'A');
            CREATE TABLE receiver_log (
                id INTEGER PRIMARY KEY, at TEXT NOT NULL, entry INTEGER NOT NULL,
                stamp TEXT, message TEXT NOT NULL,
                receiver_id INTEGER REFERENCES receiver(id),
                UNIQUE (receiver_id, entry, stamp, message)
            );
            INSERT INTO receiver_log (at, entry, stamp, message, receiver_id) VALUES
                ('2026-09-01T00:00:03Z', 3, '20050528.00:02:00', 'GPS valid', 1),
                ('2026-09-01T00:00:04Z', 2, '20050528.00:00:28', 'Position hold', 1),
                ('2026-09-01T00:00:05Z', 1, '20050528.00:00:00', 'Power on',   1),
                ('2026-09-02T00:00:01Z', 1, '20050530.00:00:00', 'Power on',   1),
                ('2026-09-02T00:00:02Z', 2, '20050530.00:00:31', 'Position hold', 1);
            "#,
        )
        .expect("write the old shape");
        drop(old);

        let log = Log::open(&path).expect("migrate it");
        let rows: Vec<(i64, i64)> = log
            .conn
            .prepare("SELECT generation, entry FROM receiver_log ORDER BY generation, entry")
            .expect("prepare")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query")
            .collect::<std::result::Result<_, _>>()
            .expect("collect");
        assert_eq!(rows, vec![(0, 1), (0, 2), (0, 3), (1, 1), (1, 2)]);
        // Nothing was dropped on the way through.
        assert_eq!(log.journal_counts().expect("count them").1, 5);
        assert_eq!(log.log_generation(1).expect("the newest generation"), 1);
        assert_eq!(log.log_span(1, 0).expect("the old span"), Some((1, 3)));
        assert_eq!(log.log_span(1, 1).expect("the new span"), Some((1, 2)));
        // The old generation's three entries must not make the new
        // one's two look like a complete log of five.
        assert!(log.log_complete(1, 1, 2).expect("complete"));
        assert!(!log.log_complete(1, 1, 3).expect("not complete"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_diagnostic_log_entry_is_not_recorded_twice() {
        // The copy walks the log whenever the entry count is in doubt,
        // so it re-reads entries it already holds as a matter of
        // course. Without this the same entry accumulates a row per
        // pass, for ever.
        let path = scratch("entries");
        let mut log = Log::open(&path).expect("open the database");
        log.note_receiver("HEWLETT-PACKARD,58503A,A,3704-C")
            .expect("note the receiver");
        let entry = |log: &mut Log, generation, n, stamp, message| {
            log.record_log_entry(generation, n, Some(stamp), message)
                .expect("record an entry")
        };

        assert!(entry(
            &mut log,
            0,
            7,
            "20050727.06:17:34",
            "Holdover started"
        ));
        assert!(!entry(
            &mut log,
            0,
            7,
            "20050727.06:17:34",
            "Holdover started"
        ));
        // Within a generation the entry number is the receiver's own
        // key, so a re-read that disagrees about the text is still the
        // same entry and must not make a second row.
        assert!(!entry(
            &mut log,
            0,
            7,
            "20050728.06:17:34",
            "Holdover ended"
        ));
        // A cleared log numbers again from one.  That is a different
        // entry, and the generation is what says so.
        assert!(entry(
            &mut log,
            1,
            7,
            "20050728.06:17:34",
            "Holdover started"
        ));
        assert_eq!(log.journal_counts().expect("count them").1, 2);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rows_say_which_receiver_they_came_from() {
        // A bench where units are swapped puts two receivers' history
        // in one file.  Knowing that two wrote is not enough: without a
        // per-row answer, every long-run comparison in the file is
        // between two different oscillators and reads as one drifting.
        let path = scratch("receivers");
        let mut log = Log::open(&path).expect("open the database");
        let note =
            |log: &mut Log, identity| log.note_receiver(identity).expect("note the receiver");

        assert!(note(&mut log, "HEWLETT-PACKARD,58503A,A,3704-C").is_empty());
        log.record_error(-313, "first unit").expect("record");
        // A firmware upgrade is the same receiver, and saying otherwise
        // would cry wolf on every upgrade.
        assert!(note(&mut log, "HEWLETT-PACKARD,58503A,A,3714-C").is_empty());
        log.record_error(-314, "same unit, new firmware")
            .expect("record");
        // A different serial is a different receiver.
        assert_eq!(
            note(&mut log, "HEWLETT-PACKARD,58503A,B,3704-C"),
            vec!["A".to_owned()]
        );
        log.record_error(-315, "second unit").expect("record");

        assert_eq!(
            log.receivers().expect("list them"),
            vec![
                ("A".to_owned(), "58503A".to_owned()),
                ("B".to_owned(), "58503A".to_owned()),
            ]
        );
        // Three errors, two units, and each error knows which.
        let by_unit: Vec<(String, String)> = log
            .conn
            .prepare(
                "SELECT receiver.serial, receiver_error.message
                 FROM receiver_error JOIN receiver ON receiver.id = receiver_error.receiver_id
                 ORDER BY receiver_error.id",
            )
            .expect("prepare")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query")
            .collect::<std::result::Result<_, _>>()
            .expect("collect");
        assert_eq!(
            by_unit,
            vec![
                ("A".to_owned(), "first unit".to_owned()),
                ("A".to_owned(), "same unit, new firmware".to_owned()),
                ("B".to_owned(), "second unit".to_owned()),
            ]
        );
        // And the firmware recorded is the one now on the unit.
        let firmware: String = log
            .conn
            .query_row(
                "SELECT firmware FROM receiver WHERE serial = 'A'",
                [],
                |row| row.get(0),
            )
            .expect("read the firmware");
        assert_eq!(firmware, "3714-C");
        let _ = std::fs::remove_file(&path);
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

        // Nor is anything built in it.  The version check used to run
        // after the whole migration, so a database from a future daemon
        // had three tables created and seven columns added to it and
        // was then refused -- and a later schema that renamed one of
        // those would have found it quietly resurrected.  Refusing has
        // to mean touching nothing.
        let mut built: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
            .expect("read the schema")
            .query_map([], |row| row.get(0))
            .expect("list the tables")
            .collect::<std::result::Result<_, _>>()
            .expect("collect the tables");
        built.sort();
        assert_eq!(
            built,
            vec!["meta".to_owned()],
            "a refused database must be left as it was found"
        );
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
