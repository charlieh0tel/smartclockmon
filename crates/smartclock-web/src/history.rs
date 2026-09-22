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
        let mut rows = statement.query((from, to, points, span))?;
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
    pub(crate) fn extent(&self) -> Result<(f64, f64)> {
        let extent = self.conn.query_row(
            "SELECT unixepoch(MIN(at)), unixepoch(MAX(at)) FROM snapshot",
            [],
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
    pub(crate) fn journal(&self, limit: usize) -> Result<Journal> {
        let limit = limit.min(MAX_JOURNAL) as i64;
        Ok(Journal {
            // Ordered by the receiver's own clock, not by when we
            // copied each entry out.  The backfill walks newest to
            // oldest, so copy order is reverse chronology; and the
            // entry number restarts at one whenever the log is
            // cleared, so that is no better.  The calendar runs across
            // a clear, which leaves its own stamp as the only key that
            // orders the whole log.
            entries: self.stream(
                "SELECT at, entry, stamp, message FROM receiver_log
                 ORDER BY COALESCE(stamp, at) DESC",
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
                "SELECT at, register, bits, decoded FROM receiver_event ORDER BY id DESC",
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
                "SELECT at, code, message FROM receiver_error ORDER BY id DESC",
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
    fn stream<T, F>(&self, select: &str, limit: i64, read: F) -> Result<Vec<T>>
    where
        F: Fn(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    {
        let sql = format!("{select} LIMIT ?1");
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
            .query_map([limit], |row| read(row))?
            .collect::<std::result::Result<_, _>>()?)
    }
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
