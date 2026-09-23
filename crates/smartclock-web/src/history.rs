//! Reading series out of the daemon's log.
//!
//! Opened read-only: the daemon is the only writer, and a reader that
//! cannot write cannot corrupt a record that exists to be trusted.
//!
//! Bucketed in SQL rather than in the browser.  A week at the fast
//! tier is about six hundred thousand rows, and sending those to a
//! phone to be thrown away there would be slow in the one place the
//! page has no cycles to spare.

use anyhow::Context as _;
use anyhow::Result;
use rusqlite::Connection;
use rusqlite::OpenFlags;

/// A column of the snapshot table that can be plotted.
///
/// Named here rather than taken from the query string, so a request
/// cannot ask for arbitrary SQL.
///
/// The order is deliberate and doing two jobs: it is the order of the
/// menu on the page, and the order of the stack, so that ticking a
/// series does not reshuffle the plots already drawn.  The four the
/// oscillator's behaviour is read from come first, in the order they
/// are usually read in -- what the clock is actually doing, what the
/// loop is doing about it, and the two things that move it.
pub(crate) const PLOTTABLE: [&str; 8] = [
    "time_interval_s",
    "efc_percent",
    "temperature_c",
    "oven_current",
    "efc_dac",
    "oven_tempco",
    "tfom",
    "ffom",
];

/// The most series one request will bucket together.
///
/// Each adds three aggregates to the same query and a chart to the
/// page; past a handful the page is taller than a screen and the point
/// of stacking them -- seeing them against one another -- is lost.
const MAX_SERIES: usize = 6;

/// Fewest buckets worth drawing, and the most a chart can show.
///
/// The upper bound is about three times the pixels across a wide
/// screen, so asking for more cannot make the picture better and can
/// make the query slow.
const MIN_POINTS: usize = 16;
const MAX_POINTS: usize = 5000;

/// One bucket: when, and what the readings in it did.
///
/// `min` and `max` are kept because a mean alone hides the thing worth
/// seeing.  A step that lasted a second inside a ten minute bucket
/// moves the mean imperceptibly and the max completely.
#[derive(Debug, serde::Serialize)]
pub(crate) struct Series {
    /// The bucket centres, shared by every plot below.
    ///
    /// One array, not one per plot: it is what makes the stacked
    /// charts line up, and sending it once says so.
    pub(crate) at: Vec<f64>,
    /// One per column asked for, in the order asked.
    pub(crate) plots: Vec<Plot>,
}

/// One column's buckets, against [`Series::at`].
///
/// `min` and `max` are kept because a mean alone hides the thing worth
/// seeing.  A step that lasted a second inside a ten minute bucket
/// moves the mean imperceptibly and the max completely.
#[derive(Debug, serde::Serialize)]
pub(crate) struct Plot {
    /// Which column this is.
    pub(crate) column: String,
    pub(crate) mean: Vec<Option<f64>>,
    pub(crate) min: Vec<Option<f64>>,
    pub(crate) max: Vec<Option<f64>>,
}

/// The daemon's log, open for reading.
#[derive(Debug)]
pub(crate) struct Log {
    conn: Connection,
}

impl Log {
    pub(crate) fn open(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .with_context(|| format!("opening {} read-only", path.display()))?;
        Ok(Self { conn })
    }

    /// Every receiver this log holds, newest first by when it was
    /// last seen.
    ///
    /// Empty on a log written before receivers were recorded, which is
    /// not an error: such a log has one unit's rows and no name for it.
    pub(crate) fn receivers(&self) -> Result<Vec<Receiver>> {
        let mut statement = match self.conn.prepare(
            "SELECT id, serial, COALESCE(model, ''), COALESCE(firmware, ''),
                    COALESCE(first_seen, ''), COALESCE(last_seen, '')
             FROM receiver ORDER BY last_seen DESC",
        ) {
            Ok(statement) => statement,
            Err(rusqlite::Error::SqliteFailure(_, Some(ref why)))
                if why.contains("no such table") =>
            {
                return Ok(Vec::new());
            }
            Err(e) => return Err(e.into()),
        };
        Ok(statement
            .query_map([], |row| {
                Ok(Receiver {
                    id: row.get(0)?,
                    serial: row.get(1)?,
                    model: row.get(2)?,
                    firmware: row.get(3)?,
                    first_seen: row.get(4)?,
                    last_seen: row.get(5)?,
                })
            })?
            .collect::<std::result::Result<_, _>>()?)
    }

    /// Which receiver a request that does not say gets.
    ///
    /// The one seen most recently, which on a bench with one unit is
    /// the only one and on a bench where they are swapped is the one
    /// attached now.  `None` when the log names none, and then nothing
    /// can be filtered and nothing should be.
    pub(crate) fn newest_receiver(&self) -> Result<Option<i64>> {
        Ok(self.receivers()?.first().map(|r| r.id))
    }

    /// Bucketed series for several columns at once, between two unix
    /// times and into at most `points` buckets each.
    ///
    ///
    /// One query rather than one per column, and that is a correctness
    /// requirement rather than an optimisation.  Stacked charts are
    /// only comparable if their x values are identical, and separate
    /// queries do not guarantee that: each would compute its own
    /// bucket boundaries from its own `to`, and two requests a second
    /// apart land on a different grid.  Bucketing every column in the
    /// same pass makes the alignment structural.
    ///
    /// Columns are checked against [`PLOTTABLE`] before reaching the
    /// SQL, which is interpolated, so a request cannot name arbitrary
    /// expressions.
    ///
    /// Rows where the fast tier did not run are left out: every
    /// plottable column is a fast-tier field, so such a row restates
    /// the previous one and plotting it draws a measurement that was
    /// never taken.  Older rows have no `fast_at` and fall back to the
    /// freshness flag.
    pub(crate) fn series(
        &self,
        receiver: i64,
        columns: &[String],
        from: i64,
        to: i64,
        points: usize,
    ) -> Result<Series> {
        anyhow::ensure!(!columns.is_empty(), "no columns asked for");
        anyhow::ensure!(
            columns.len() <= MAX_SERIES,
            "at most {MAX_SERIES} series at once"
        );
        for column in columns {
            anyhow::ensure!(
                PLOTTABLE.contains(&column.as_str()),
                "{column} is not a column this serves"
            );
        }
        let points = points.clamp(MIN_POINTS, MAX_POINTS) as i64;
        anyhow::ensure!(from <= to, "the range ends before it starts");
        let span = to.saturating_sub(from).max(1);
        let aggregates = columns
            .iter()
            .map(|c| format!("AVG({c}), MIN({c}), MAX({c})"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            // Divided by the span plus one so that a row landing exactly
            // on `to` falls in the last bucket rather than in one past
            // it: the inclusive range would otherwise return points + 1
            // buckets, which is not what the caller asked for.
            "SELECT CAST((unixepoch(at) - ?1) * ?3 / (?4 + 1) AS INTEGER) AS bucket,
                    AVG(unixepoch(at)) AS at,
                    {aggregates}
             FROM snapshot
             -- Compared as text, against the column itself, so the
             -- index on `at` can be used.  unixepoch(at) >= ? reads
             -- every row in the table instead: 2 ms today, but the log
             -- grows without bound and this is one query per chart.
             -- The stored form is RFC 3339 with fractional seconds and
             -- a Z, so a bound truncated to the second sorts before
             -- every row within that second, which is what the
             -- inclusive lower and exclusive upper bounds want.
             WHERE at >= strftime('%Y-%m-%dT%H:%M:%S', ?1, 'unixepoch')
               AND at < strftime('%Y-%m-%dT%H:%M:%S', ?2 + 1, 'unixepoch')
               AND (fast_at = at OR (fast_at IS NULL AND freshness = 'live'))
               -- One unit per chart.  A bench where receivers are
               -- swapped puts two of them in one file, and a plot that
               -- ran them together would draw a step between two
               -- oscillators as though one had moved.
               AND receiver_id = ?5
             GROUP BY bucket
             ORDER BY at"
        );
        let mut statement = self.conn.prepare(&sql)?;
        let mut at = Vec::new();
        let mut plots: Vec<Plot> = columns
            .iter()
            .map(|column| Plot {
                column: column.clone(),
                mean: Vec::new(),
                min: Vec::new(),
                max: Vec::new(),
            })
            .collect();
        let mut rows = statement.query((from, to, points, span, receiver))?;
        while let Some(row) = rows.next()? {
            at.push(row.get::<_, f64>(1)?);
            for (n, plot) in plots.iter_mut().enumerate() {
                // Three aggregates per column, after bucket and at.
                let base = 2 + n * 3;
                plot.mean.push(row.get(base)?);
                plot.min.push(row.get(base + 1)?);
                plot.max.push(row.get(base + 2)?);
            }
        }
        Ok(Series { at, plots })
    }

    /// The oldest and newest readings, so the page can offer a range
    /// that exists rather than one that might not.
    pub(crate) fn extent(&self, receiver: i64) -> Result<(f64, f64)> {
        let extent = self.conn.query_row(
            "SELECT unixepoch(MIN(at)), unixepoch(MAX(at)) FROM snapshot
             WHERE receiver_id = ?1",
            [receiver],
            |row| Ok((row.get::<_, Option<f64>>(0)?, row.get::<_, Option<f64>>(1)?)),
        )?;
        Ok(match extent {
            (Some(first), Some(last)) => (first, last),
            _ => (0.0, 0.0),
        })
    }

    /// The receiver's own record-keeping, newest first.
    ///
    /// Two streams shown together because they answer the same
    /// question from different sides: the diagnostic log is what the
    /// receiver thought worth writing down, and the event rows are
    /// transitions it latched and we took.  Neither is in the snapshot
    /// table and neither can be plotted, so they would otherwise be
    /// invisible to anyone not holding a SQL prompt.
    pub(crate) fn journal(&self, receiver: i64, limit: usize) -> Result<Journal> {
        let limit = limit.min(MAX_JOURNAL) as i64;
        Ok(Journal {
            // Ordered by generation and then entry number, which is
            // the receiver's own sequence.  Not by stamp: before the
            // first GPS lock the receiver stamps entries with elapsed
            // time since boot on a stale date, so every power-on sorts
            // to the start of that day and boot sessions interleave.
            // Not by copy time either: the backfill walks newest to
            // oldest, so copy order is reverse chronology.  The entry
            // number restarts at one on a clear, which is what the
            // generation counts.
            entries: self.stream(
                receiver,
                "SELECT at, entry, stamp, message FROM receiver_log
                 WHERE receiver_id = ?1
                 ORDER BY generation DESC, entry DESC",
                limit,
                |row| {
                    Ok(Entry {
                        at: row.get(0)?,
                        entry: row.get(1)?,
                        stamp: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                        message: row.get(3)?,
                    })
                },
            )?,
            events: self.stream(
                receiver,
                "SELECT at, register, bits, decoded FROM receiver_event
                 WHERE receiver_id = ?1 ORDER BY id DESC",
                limit,
                |row| {
                    Ok(Event {
                        at: row.get(0)?,
                        register: row.get(1)?,
                        bits: row.get(2)?,
                        decoded: row.get(3)?,
                    })
                },
            )?,
            errors: self.stream(
                receiver,
                "SELECT at, code, message FROM receiver_error
                 WHERE receiver_id = ?1 ORDER BY id DESC",
                limit,
                |row| {
                    Ok(ReceiverError {
                        at: row.get(0)?,
                        code: row.get(1)?,
                        message: row.get(2)?,
                    })
                },
            )?,
        })
    }

    /// One journal stream, newest first, empty if this log predates it.
    ///
    /// A reader is not a writer and must not insist the file be as new
    /// as itself.  The daemon refuses a database from a later schema,
    /// because writing into one it does not understand could corrupt
    /// the only copy of a receiver's history; a viewer pointed at an
    /// older log, or at an archived one, should show what is there and
    /// say nothing about the rest.  Anything else means an upgraded
    /// web view breaks against a daemon not yet restarted.
    fn stream<T, F>(&self, receiver: i64, select: &str, limit: i64, read: F) -> Result<Vec<T>>
    where
        F: Fn(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    {
        let sql = format!("{select} LIMIT ?2");
        let mut statement = match self.conn.prepare(&sql) {
            Ok(statement) => statement,
            Err(rusqlite::Error::SqliteFailure(_, Some(ref why)))
                if why.contains("no such table") =>
            {
                return Ok(Vec::new());
            }
            Err(e) => return Err(e.into()),
        };
        Ok(statement
            .query_map((receiver, limit), |row| read(row))?
            .collect::<std::result::Result<_, _>>()?)
    }
}

/// One receiver the log holds rows for.
#[derive(Debug, serde::Serialize)]
pub(crate) struct Receiver {
    /// The `receiver_id` every row of this unit's carries.
    pub(crate) id: i64,
    /// What the unit calls itself, and the only field it is known by:
    /// a firmware upgrade must not make it a different receiver.
    pub(crate) serial: String,
    pub(crate) model: String,
    pub(crate) firmware: String,
    pub(crate) first_seen: String,
    pub(crate) last_seen: String,
}

/// The most of each stream one request will return.
const MAX_JOURNAL: usize = 200;

/// What the receiver has recorded about itself.
#[derive(Debug, serde::Serialize)]
pub(crate) struct Journal {
    /// Its diagnostic log, as copied out.
    pub(crate) entries: Vec<Entry>,
    /// Transitions taken from its event registers.
    pub(crate) events: Vec<Event>,
    /// Entries taken from its error queue.
    pub(crate) errors: Vec<ReceiverError>,
}

/// One diagnostic log entry.
#[derive(Debug, serde::Serialize)]
pub(crate) struct Entry {
    /// When it was copied out.
    pub(crate) at: String,
    /// The receiver's own entry number.
    pub(crate) entry: i64,
    /// Its own timestamp, as written -- on the receiver's calendar,
    /// which may be behind by whole GPS epochs.
    pub(crate) stamp: String,
    /// What it says.
    pub(crate) message: String,
}

/// One latched transition.
#[derive(Debug, serde::Serialize)]
pub(crate) struct Event {
    /// When it was read, which is within one journal pass of when it
    /// happened.
    pub(crate) at: String,
    /// Which register it came from.
    pub(crate) register: String,
    /// The raw word.
    pub(crate) bits: i64,
    /// Its bits named.
    pub(crate) decoded: String,
}

/// One entry from the error queue.
#[derive(Debug, serde::Serialize)]
pub(crate) struct ReceiverError {
    /// When it was read.  The queue carries no timestamps of its own.
    pub(crate) at: String,
    /// SCPI error code.
    pub(crate) code: i64,
    /// What the receiver called it.
    pub(crate) message: String,
}

#[cfg(test)]
mod tests {
    use super::Log;
    use rusqlite::Connection;

    /// A log holding two receivers, each with its own snapshots, log
    /// entries, events and errors.
    ///
    /// Written as raw SQL rather than through the daemon so the reader
    /// is tested against the shape it actually meets on disk, and so a
    /// change to the writer that forgets the reader shows up here.
    fn two_units(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("smartclock-web-{name}.sqlite"));
        let _ = std::fs::remove_file(&path);
        let conn = Connection::open(&path).expect("make a log");
        conn.execute_batch(
            r#"
            CREATE TABLE receiver (
                id INTEGER PRIMARY KEY, serial TEXT UNIQUE NOT NULL,
                manufacturer TEXT, model TEXT, firmware TEXT,
                first_seen TEXT, last_seen TEXT);
            INSERT INTO receiver VALUES
                (1,'AAA','HEWLETT-PACKARD','58503A','3704-C',
                 '2026-09-01T00:00:00Z','2026-09-01T02:00:00Z'),
                (2,'BBB','HEWLETT-PACKARD','Z3805A','3611-A',
                 '2026-09-02T00:00:00Z','2026-09-02T02:00:00Z');

            CREATE TABLE snapshot (
                id INTEGER PRIMARY KEY, at TEXT NOT NULL, freshness TEXT,
                fast_at TEXT, efc_percent REAL, temperature_c REAL,
                receiver_id INTEGER);
            INSERT INTO snapshot (at, freshness, fast_at, efc_percent, receiver_id) VALUES
                ('2026-09-01T00:00:00Z','live','2026-09-01T00:00:00Z', 10.0, 1),
                ('2026-09-01T00:00:01Z','live','2026-09-01T00:00:01Z', 11.0, 1),
                ('2026-09-02T00:00:00Z','live','2026-09-02T00:00:00Z', 90.0, 2),
                ('2026-09-02T00:00:01Z','live','2026-09-02T00:00:01Z', 91.0, 2);

            CREATE TABLE receiver_log (
                id INTEGER PRIMARY KEY, at TEXT NOT NULL, entry INTEGER NOT NULL,
                stamp TEXT, message TEXT NOT NULL, receiver_id INTEGER,
                generation INTEGER NOT NULL DEFAULT 0);
            -- Out of entry order on purpose, and with a power-on whose
            -- stamp is midnight on a stale date: ordering by stamp puts
            -- entry 2 last, which is the bug this ordering replaced.
            INSERT INTO receiver_log (at, entry, stamp, message, receiver_id, generation) VALUES
                ('2026-09-01T00:00:00Z',1,'20050528.00:01:00','one A',  1,0),
                ('2026-09-01T00:00:01Z',2,'20050528.00:00:00','Power on',1,0),
                ('2026-09-01T00:00:02Z',1,'20050530.00:00:00','after clear A',1,1),
                ('2026-09-02T00:00:00Z',1,'20050601.00:00:00','one B',  2,0);

            CREATE TABLE receiver_event (
                id INTEGER PRIMARY KEY, at TEXT NOT NULL, register TEXT,
                bits INTEGER, decoded TEXT, receiver_id INTEGER);
            INSERT INTO receiver_event (at, register, bits, decoded, receiver_id) VALUES
                ('2026-09-01T00:00:00Z','alarm',0,'clear',1),
                ('2026-09-02T00:00:00Z','alarm',8,'holdover',2);

            CREATE TABLE receiver_error (
                id INTEGER PRIMARY KEY, at TEXT NOT NULL, code INTEGER,
                message TEXT, receiver_id INTEGER);
            INSERT INTO receiver_error (at, code, message, receiver_id) VALUES
                ('2026-09-01T00:00:00Z',-113,'undefined header',1),
                ('2026-09-02T00:00:00Z',-230,'data corrupt or stale',2);
            "#,
        )
        .expect("fill it");
        path
    }

    #[test]
    fn a_plot_shows_one_receiver_and_not_the_other() {
        // The bug this exists for: every query read the whole table,
        // so two units' readings were drawn as one trace and a swap
        // looked like an oscillator stepping.
        let path = two_units("plot");
        let log = Log::open(&path).expect("open");
        let columns = vec!["efc_percent".to_owned()];
        // The window is every row either unit has, so anything left
        // out was left out by the filter and not by the range.
        let (first, last) = {
            let (fa, la) = log.extent(1).expect("A's extent");
            let (fb, lb) = log.extent(2).expect("B's extent");
            (fa.min(fb) as i64, la.max(lb) as i64)
        };
        let a = log.series(1, &columns, first, last, 100).expect("unit A");
        let b = log.series(2, &columns, first, last, 100).expect("unit B");
        // The window spans both units' days, so each one's two rows
        // fall in a single bucket: the mean of that bucket is the test.
        // Contamination could not hide in it -- mixing A's 10 and 11
        // with B's 90 and 91 gives 50.5, not 10.5.
        let span = |s: &super::Series| {
            let plot = &s.plots[0];
            (
                plot.mean.iter().flatten().copied().collect::<Vec<_>>(),
                plot.min.iter().flatten().copied().collect::<Vec<_>>(),
                plot.max.iter().flatten().copied().collect::<Vec<_>>(),
            )
        };
        assert_eq!(
            span(&a),
            (vec![10.5], vec![10.0], vec![11.0]),
            "A's readings only"
        );
        assert_eq!(
            span(&b),
            (vec![90.5], vec![90.0], vec![91.0]),
            "B's readings only"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_extent_is_the_chosen_receivers_own() {
        // A shared extent would open the page on a window in which the
        // selected unit has nothing, which reads as a dead receiver.
        let path = two_units("extent");
        let log = Log::open(&path).expect("open");
        let (first_a, last_a) = log.extent(1).expect("A");
        let (first_b, _) = log.extent(2).expect("B");
        assert!(last_a < first_b, "A's history ends before B's begins");
        assert!(first_a < last_a);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_journal_is_one_receivers_and_in_the_receivers_own_order() {
        let path = two_units("journal");
        let log = Log::open(&path).expect("open");
        let a = log.journal(1, 50).expect("A's journal");
        assert_eq!(
            a.entries
                .iter()
                .map(|e| e.message.as_str())
                .collect::<Vec<_>>(),
            vec!["after clear A", "Power on", "one A"],
            "newest generation first, then by entry number, not by stamp"
        );
        assert_eq!(a.events.len(), 1);
        assert_eq!(a.errors.len(), 1);
        assert_eq!(a.errors[0].code, -113);

        let b = log.journal(2, 50).expect("B's journal");
        assert_eq!(
            b.entries
                .iter()
                .map(|e| e.message.as_str())
                .collect::<Vec<_>>(),
            vec!["one B"]
        );
        assert_eq!(b.errors[0].code, -230);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_default_receiver_is_the_one_seen_most_recently() {
        let path = two_units("newest");
        let log = Log::open(&path).expect("open");
        assert_eq!(log.newest_receiver().expect("newest"), Some(2));
        let names: Vec<String> = log
            .receivers()
            .expect("list")
            .into_iter()
            .map(|r| r.serial)
            .collect();
        assert_eq!(names, vec!["BBB".to_owned(), "AAA".to_owned()]);
        let _ = std::fs::remove_file(&path);
    }
}
