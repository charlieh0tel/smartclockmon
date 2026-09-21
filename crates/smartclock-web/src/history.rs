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
pub(crate) const PLOTTABLE: [&str; 7] = [
    "efc_percent",
    "efc_dac",
    "temperature_c",
    "oven_current",
    "time_interval_s",
    "tfom",
    "ffom",
];

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
#[derive(Debug)]
pub(crate) struct Bucket {
    pub(crate) at: f64,
    pub(crate) mean: Option<f64>,
    pub(crate) min: Option<f64>,
    pub(crate) max: Option<f64>,
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

    /// Bucket `column` between two unix times into at most `points`.
    ///
    /// Rows where the fast tier did not run are left out: every
    /// plottable column is a fast-tier field, so such a row restates
    /// the previous one and plotting it draws a measurement that was
    /// never taken.  Older rows have no `fast_at` and fall back to the
    /// freshness flag.
    pub(crate) fn series(
        &self,
        column: &str,
        from: i64,
        to: i64,
        points: usize,
    ) -> Result<Vec<Bucket>> {
        anyhow::ensure!(
            PLOTTABLE.contains(&column),
            "{column} is not a column this serves"
        );
        let points = points.clamp(MIN_POINTS, MAX_POINTS) as i64;
        anyhow::ensure!(from <= to, "the range ends before it starts");
        let span = to.saturating_sub(from).max(1);
        let sql = format!(
            "SELECT CAST((unixepoch(at) - ?1) * ?3 / ?4 AS INTEGER) AS bucket,
                    AVG(unixepoch(at)) AS at,
                    AVG({column}), MIN({column}), MAX({column})
             FROM snapshot
             WHERE unixepoch(at) >= ?1 AND unixepoch(at) <= ?2
               AND (fast_at = at OR (fast_at IS NULL AND freshness = 'live'))
             GROUP BY bucket
             ORDER BY at"
        );
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map((from, to, points, span), |row| {
            Ok(Bucket {
                at: row.get::<_, f64>(1)?,
                mean: row.get(2)?,
                min: row.get(3)?,
                max: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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
}
