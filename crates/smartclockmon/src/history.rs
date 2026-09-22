//! Reading the daemon's log.
//!
//! Opened read-only.  The daemon holds the one write connection and
//! runs the database in WAL, so a reader runs alongside it without
//! coordination and without holding it up.

use std::path::Path;

use anyhow::Context as _;
use anyhow::Result;
use rusqlite::Connection;
use rusqlite::OpenFlags;

/// How far back a graph looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Window {
    /// The last hour.
    Hour,
    /// The last day.
    Day,
    /// The last week.
    Week,
}

impl Window {
    /// Cycle to the next span.
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Hour => Self::Day,
            Self::Day => Self::Week,
            Self::Week => Self::Hour,
        }
    }

    /// A label for the pane title.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Hour => "1 hour",
            Self::Day => "24 hours",
            Self::Week => "7 days",
        }
    }

    /// How many seconds back this reaches.
    fn seconds(self) -> i64 {
        match self {
            Self::Hour => 3600,
            Self::Day => 86_400,
            Self::Week => 7 * 86_400,
        }
    }
}

/// One series, as points of seconds-ago against value.
///
/// Time runs negative into the past so a chart's x axis reads left to
/// right with now at the right edge.
pub(crate) type Series = Vec<(f64, f64)>;

/// One metric over a window: the mean per column, and the extremes.
///
/// The extremes are not decoration.  A week of readings is thinned to a
/// few hundred columns, and a mean alone turns structure into a smooth
/// ramp.  The 1 PPS interval is the case that matters: it sits still
/// and then steps, which is what quantization looks like, and averaging
/// a bucket that contains a step reports neither the value before nor
/// the value after.
#[derive(Debug, Default)]
pub(crate) struct Trace {
    /// Mean of each column.
    pub(crate) mean: Series,
    /// Lowest reading in each column.
    pub(crate) low: Series,
    /// Highest reading in each column.
    pub(crate) high: Series,
}

impl Trace {
    /// The span the band covers, padded so a flat trace is not drawn on
    /// the axis itself.
    pub(crate) fn bounds(&self) -> Option<[f64; 2]> {
        let mut values = self.low.iter().chain(self.high.iter()).map(|(_, v)| *v);
        let first = values.next()?;
        let (lo, hi) = values.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v)));
        let pad = ((hi - lo) * 0.1).max(f64::EPSILON);
        Some([lo - pad, hi + pad])
    }

    /// The span of time covered.
    pub(crate) fn span(&self) -> [f64; 2] {
        [
            self.mean.first().map_or(-1.0, |(t, _)| *t),
            self.mean.last().map_or(0.0, |(t, _)| *t),
        ]
    }

    fn push(&mut self, ago: f64, mean: Option<f64>, low: Option<f64>, high: Option<f64>) {
        if let (Some(mean), Some(low), Some(high)) = (mean, low, high) {
            self.mean.push((ago, mean));
            self.low.push((ago, low));
            self.high.push((ago, high));
        }
    }

    /// Scale every value, for a unit change.
    fn scale(&mut self, factor: f64) {
        for series in [&mut self.mean, &mut self.low, &mut self.high] {
            for point in series.iter_mut() {
                point.1 *= factor;
            }
        }
    }
}

/// What a graph needs, over one window.
#[derive(Debug, Default)]
pub(crate) struct History {
    /// Oscillator control voltage, percent.
    pub(crate) efc: Trace,
    /// Internal temperature, degrees Celsius.
    pub(crate) temperature: Trace,
    /// 1 PPS interval against GPS, nanoseconds.
    pub(crate) time_interval: Trace,
    /// How many snapshots the window covered.
    pub(crate) rows: usize,
}

/// The most notes one read will return.
const MAX_JOURNAL: usize = 500;

/// Which of the receiver's records a note came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// The receiver's own diagnostic log.
    Log,
    /// A transition latched in an event register.
    Event,
    /// An entry from the error queue.
    Error,
}

impl Source {
    /// A short tag for the column that says where a note came from.
    pub(crate) fn tag(self) -> &'static str {
        match self {
            Self::Log => "log",
            Self::Event => "event",
            Self::Error => "error",
        }
    }
}

/// One thing the receiver recorded about itself.
#[derive(Debug, Clone)]
pub(crate) struct Note {
    /// When the daemon read it, on the host clock.
    ///
    /// The sort key, and only that.  Ordering the three records against
    /// each other needs one clock, and this is the only one they share:
    /// a diagnostic log entry carries the receiver's calendar, which on
    /// this firmware is 1024 weeks behind, so comparing those against
    /// host timestamps puts every entry from the receiver's log after
    /// every event regardless of when either happened.
    pub(crate) at: String,
    /// What to show, which is the receiver's own stamp where it has
    /// one.
    ///
    /// Separate from the sort key because the two answer different
    /// questions.  An entry the receiver timestamped is better
    /// displayed by its own clock -- that is the string that appears in
    /// the instrument and the one worth searching for -- while the
    /// order it belongs in is the order we learned it.
    pub(crate) stamp: String,
    /// What it says.
    pub(crate) text: String,
    /// Which record it came from.
    pub(crate) source: Source,
}

/// A read-only view of the daemon's log.
#[derive(Debug)]
pub(crate) struct Log {
    conn: Connection,
}

impl Log {
    /// Open the log without taking a write lock on it.
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .with_context(|| format!("opening {} read-only", path.display()))?;
        Ok(Self { conn })
    }

    /// The receiver's own record-keeping.
    ///
    /// Grouped, not interleaved.  The three records run on two clocks:
    /// events and errors are stamped when the daemon read them, while
    /// a diagnostic log entry carries the receiver's own calendar,
    /// 1024 weeks behind on this firmware.  Merging them into one
    /// chronology needs a common clock, and the only one available --
    /// when we read it -- puts the log in the order it was *copied*,
    /// which for a backfill that walks newest-to-oldest is precisely
    /// backwards.  So each record keeps its own order and they are
    /// shown in sequence: what has happened recently first, then the
    /// receiver's own history in its own sequence.
    pub(crate) fn journal(&self, limit: usize) -> Result<Vec<Note>> {
        let limit = limit.min(MAX_JOURNAL) as i64;
        let mut notes = Vec::new();
        // The receiver's own stamp where there is one: an entry it
        // timestamped itself is better dated by the receiver than by
        // when we happened to copy it out, even though that clock can
        // be behind by whole GPS epochs.
        // Events and errors share the host clock, so these two do
        // merge, newest first.
        let mut recent = self.stream(
            "SELECT at, at, decoded FROM receiver_event",
            limit,
            Source::Event,
        )?;
        recent.extend(self.stream(
            "SELECT at, at, code || ' ' || message FROM receiver_error",
            limit,
            Source::Error,
        )?);
        recent.sort_by(|a, b| b.at.cmp(&a.at));
        notes.extend(recent);

        // Then the receiver's log on the receiver's own clock.  Not by
        // entry number, which restarts at one whenever the log is
        // cleared and would bury everything since the last clear at the
        // bottom; and not by when we copied it, which for a backfill
        // walking newest-to-oldest is exactly backwards.  The calendar
        // keeps running across a clear, so its own stamp is the only
        // key that orders the whole log correctly.
        //
        // Imperfect around a power cycle, where the clock restarts at
        // midnight on a stale date until the first lock.  Those entries
        // sort early; the entry numbers in the web view disambiguate.
        notes.extend(self.ordered(
            "SELECT at, COALESCE(stamp, at), message FROM receiver_log
             ORDER BY COALESCE(stamp, at) DESC LIMIT ?1",
            limit,
            Source::Log,
        )?);
        notes.truncate(limit as usize);
        Ok(notes)
    }

    /// One stream, empty if this log predates it.
    ///
    /// A reader is not a writer and must not require the file be as new
    /// as itself: the monitor can be upgraded before the daemon is
    /// restarted, or pointed at an archived log, and either should show
    /// what is there rather than refusing the lot.
    fn stream(&self, select: &str, limit: i64, source: Source) -> Result<Vec<Note>> {
        self.ordered(
            &format!("{select} ORDER BY id DESC LIMIT ?1"),
            limit,
            source,
        )
    }

    /// As [`Log::stream`], for a query that states its own ordering.
    fn ordered(&self, sql: &str, limit: i64, source: Source) -> Result<Vec<Note>> {
        let mut statement = match self.conn.prepare(sql) {
            Ok(statement) => statement,
            Err(rusqlite::Error::SqliteFailure(_, Some(ref why)))
                if why.contains("no such table") =>
            {
                return Ok(Vec::new());
            }
            Err(e) => return Err(e.into()),
        };
        Ok(statement
            .query_map([limit], |row| {
                Ok(Note {
                    at: row.get(0)?,
                    stamp: row.get(1)?,
                    text: row.get(2)?,
                    source,
                })
            })?
            .collect::<std::result::Result<_, _>>()?)
    }

    /// Read the series a graph needs.
    ///
    /// Rows are thinned to at most `columns` points, since a week at one
    /// second is six hundred thousand rows and a terminal has a couple
    /// of hundred columns.  Thinning happens in SQL so the rows never
    /// cross the process boundary.
    ///
    /// The window is compared as a number, not as text.  `at` is
    /// written by jiff as `2026-09-20T00:05:00.123456789Z` with a
    /// `T`, while `datetime('now')` yields `2026-09-20 23:07:40`
    /// with a space, and `'T'` sorts after `' '`.  A lexical
    /// comparison therefore passed every row whose date matched, so
    /// the pane titled "1 hour" could show two days.  Measured
    /// against the development log: 3133 rows returned for the last
    /// hour where 2545 was correct.
    pub(crate) fn read(&self, window: Window, columns: usize) -> Result<History> {
        let span = window.seconds();
        let buckets = columns.clamp(16, 1024) as i64;
        // Bucket by time so each column is one averaged point.  Averaging
        // rather than sampling keeps a spike from vanishing between
        // columns.
        let mut statement = self.conn.prepare(
            "SELECT
                 CAST((unixepoch(at) - unixepoch('now')) / MAX(?1 / ?2, 1) AS INTEGER) AS bucket,
                 AVG(unixepoch(at) - unixepoch('now')) AS ago,
                 AVG(efc_percent),       MIN(efc_percent),       MAX(efc_percent),
                 AVG(temperature_c),     MIN(temperature_c),     MAX(temperature_c),
                 AVG(time_interval_s),   MIN(time_interval_s),   MAX(time_interval_s)
             FROM snapshot
             WHERE unixepoch(at) >= unixepoch('now') - ?1
               -- Every column plotted here is a fast-tier field, so the
               -- question is whether the fast tier measured this row or
               -- the row merely restates the last one.  fast_at answers
               -- it exactly; the freshness flag does not, since it now
               -- reports the whole snapshot and goes Stale when some
               -- other tier fails.  Rows from before those columns
               -- existed have no fast_at and fall back to the flag.
               AND (fast_at = at OR (fast_at IS NULL AND freshness = 'live'))
             GROUP BY bucket
             ORDER BY ago",
        )?;

        let mut out = History::default();
        let rows = statement.query_map((span, buckets), |row| {
            let value = |n: usize| row.get::<_, Option<f64>>(n);
            Ok((
                row.get::<_, f64>(1)?,
                (value(2)?, value(3)?, value(4)?),
                (value(5)?, value(6)?, value(7)?),
                (value(8)?, value(9)?, value(10)?),
            ))
        })?;
        for row in rows {
            let (ago, efc, temperature, interval) = row?;
            out.rows += 1;
            out.efc.push(ago, efc.0, efc.1, efc.2);
            out.temperature
                .push(ago, temperature.0, temperature.1, temperature.2);
            out.time_interval
                .push(ago, interval.0, interval.1, interval.2);
        }
        // Nanoseconds: the interval is a few parts in a billion of a
        // second and would render as a flat zero.
        out.time_interval.scale(1e9);
        Ok(out)
    }
}
