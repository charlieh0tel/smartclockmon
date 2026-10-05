//! The tables, as the daemon creates them and the readers expect them.

use rusqlite::Connection;
use smartclock::command::CommandId;
use smartclock::device::dialect_for;
use smartclock::snapshot::Tier;

/// Bumped when the tables change shape, or what a column holds.
///
/// The daemon refuses a log of a later version rather than write into
/// what it does not understand.  The readers accept any: a viewer
/// upgraded before the daemon restarts, or pointed at an archived log,
/// shows what is there.
pub const VERSION: i64 = 10;

/// The table of facts about the log itself, the schema version first.
///
/// Created alone and before anything else, because the version it
/// holds decides whether the rest may be touched.
pub const META: &str = "CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);";

/// Every other table, as of [`VERSION`].
///
/// Each `IF NOT EXISTS`, so running this against an older log adds
/// what it lacks and leaves the rest; a column added since a table was
/// first created is the daemon's migration to add.
pub const TABLES: &str = r#"
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
    -- screen and so only on slow-tier polls.  The counts above
    -- are queried directly and move with the medium tier.
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
        -- The newest measured row's time: set on attach, then with
        -- every snapshot logged that is not a disconnection.
        last_seen    TEXT NOT NULL,
        -- The internal GPS engine's identity, exactly as
        -- :DIAGnostic:IDENtification:GPSystem? answered it, as
        -- last read.  NULL until a daemon of schema 9 or later
        -- has asked.
        gps_engine   TEXT
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
        receiver_id INTEGER REFERENCES receiver(id)
    );
    CREATE INDEX IF NOT EXISTS audit_at ON audit(at);
"#;

/// A timestamp as the log stores it: RFC 3339 in UTC, always with nine
/// fractional digits.
///
/// Fixed width so that text order is time order.  The default form
/// trims trailing zeros, and `00Z`, `00.1Z` and `00.11Z` sort the
/// wrong way round as text; every range and every `ORDER BY at` in the
/// readers compares the text.
pub fn stored(at: jiff::Timestamp) -> String {
    format!("{at:.9}")
}

/// The `meta` key the daemon records a tier's cadence under, in
/// seconds.
pub fn cadence_key(tier: Tier) -> String {
    format!("cadence_{}", tier.name())
}

/// Whether `table` has `column`, for a log that may predate it.
///
/// `table` is interpolated and must be one of ours, never caller input.
pub fn has_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut names = statement.query_map([], |row| row.get::<_, String>(1))?;
    Ok(names.any(|name| name.is_ok_and(|n| n == column)))
}

/// A column of the snapshot table that can be plotted.
///
/// Named here rather than taken from a request, so a request cannot
/// ask for arbitrary SQL.
///
/// The order is deliberate and doing two jobs: it is the order of the
/// web view's menu, and the order of its stack, so that ticking a
/// series does not reshuffle the plots already drawn.  The four the
/// oscillator's behavior is read from come first, in the order they
/// are usually read in -- what the clock is actually doing, what the
/// loop is doing about it, and the two things that move it.
///
/// `tracking` sits third because a receiver losing satellites
/// explains the two above it, and reading those without it invites
/// blaming the oscillator for the sky.
///
/// Each with the tier that reads it, which decides how long a value
/// stays current, and the command it is read with, which decides
/// whether a unit measures it at all.
pub const PLOTTABLE: [(&str, Tier, CommandId); 10] = [
    ("time_interval_s", Tier::Fast, CommandId::Tinterval),
    ("efc_percent", Tier::Fast, CommandId::Efc),
    ("tracking", Tier::Medium, CommandId::SatTrackingCount),
    ("temperature_c", Tier::Medium, CommandId::Temperature),
    ("oven_current", Tier::Medium, CommandId::OvenCurrent),
    ("efc_dac", Tier::Medium, CommandId::EfcAbsolute),
    ("oven_tempco", Tier::Slow, CommandId::OvenTempco),
    ("tfom", Tier::Fast, CommandId::Tfom),
    ("ffom", Tier::Fast, CommandId::Ffom),
    ("not_tracking", Tier::Medium, CommandId::SatVisibleCount),
];

/// The columns a unit of `model` measures: those its command tree has
/// a command for.  A column it cannot read is only ever empty, so a
/// view leaves it out rather than draw a chart of nothing.
pub fn measured(model: &str) -> Vec<&'static str> {
    let dialect = dialect_for(model);
    PLOTTABLE
        .iter()
        .filter(|(_, _, command)| dialect.spec(*command).is_some())
        .map(|(column, _, _)| *column)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::META;
    use super::PLOTTABLE;
    use super::TABLES;
    use super::has_column;
    use super::measured;
    use rusqlite::Connection;

    #[test]
    fn a_unit_is_offered_only_the_columns_it_measures() {
        let z3805 = measured("Z3805A");
        assert!(!z3805.contains(&"temperature_c"), "{z3805:?}");
        assert!(z3805.contains(&"time_interval_s"), "{z3805:?}");
        assert!(measured("58503A").contains(&"temperature_c"));
    }

    #[test]
    fn every_plottable_column_is_in_the_snapshot_table() {
        let conn = Connection::open_in_memory().expect("a database");
        conn.execute_batch(META).expect("meta");
        conn.execute_batch(TABLES).expect("the tables");
        for (column, _, _) in PLOTTABLE {
            assert!(
                has_column(&conn, "snapshot", column).expect("table info"),
                "{column} is plottable but not a column"
            );
        }
    }
}
