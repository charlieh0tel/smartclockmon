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

/// One receiver the log holds rows for.
#[derive(Debug, Clone)]
pub(crate) struct Receiver {
    /// The `receiver_id` this unit's rows carry.
    pub(crate) id: i64,
    /// Its serial number, which is the only thing it is known by.
    pub(crate) serial: String,
    pub(crate) model: String,
}

impl Receiver {
    /// How the status line names it.
    pub(crate) fn label(&self) -> String {
        if self.model.is_empty() {
            self.serial.clone()
        } else {
            format!("{} {}", self.model, self.serial)
        }
    }
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

    /// Every receiver this log holds, most recently seen first.
    ///
    /// Empty on a log written before receivers were recorded, which is
    /// not an error: such a log has one unit's rows and no name for it.
    pub(crate) fn receivers(&self) -> Result<Vec<Receiver>> {
        let mut statement = match self.conn.prepare(
            "SELECT id, serial, COALESCE(model, '') FROM receiver
             ORDER BY last_seen DESC",
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
                })
            })?
            .collect::<std::result::Result<_, _>>()?)
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
    pub(crate) fn journal(&self, receiver: i64, limit: usize) -> Result<Vec<Note>> {
        let limit = limit.min(MAX_JOURNAL) as i64;
        let mut notes = Vec::new();
        // The receiver's own stamp where there is one: an entry it
        // timestamped itself is better dated by the receiver than by
        // when we happened to copy it out, even though that clock can
        // be behind by whole GPS epochs.
        // Events and errors share the host clock, so these two do
        // merge, newest first.
        let mut recent = self.stream(
            "SELECT at, at, decoded FROM receiver_event WHERE receiver_id = ?1",
            receiver,
            limit,
            Source::Event,
        )?;
        recent.extend(self.stream(
            "SELECT at, at, code || ' ' || message FROM receiver_error
             WHERE receiver_id = ?1",
            receiver,
            limit,
            Source::Error,
        )?);
        recent.sort_by(|a, b| b.at.cmp(&a.at));
        notes.extend(recent);

        // Then the receiver's log in the receiver's own sequence:
        // newest generation first, and within it by entry number.  Not
        // by stamp, which this used to use -- before the first GPS lock
        // the receiver stamps entries with elapsed time since boot on a
        // stale date, so every power-on sorts to the start of that day
        // and boot sessions interleave.  Not by when we copied it
        // either, which for a backfill walking newest-to-oldest is
        // exactly backwards.  The generation counts the clears that
        // restart the numbering.
        notes.extend(self.ordered(
            "SELECT at, COALESCE(stamp, at), message FROM receiver_log
             WHERE receiver_id = ?1
             ORDER BY generation DESC, entry DESC LIMIT ?2",
            receiver,
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
    fn stream(&self, select: &str, receiver: i64, limit: i64, source: Source) -> Result<Vec<Note>> {
        self.ordered(
            &format!("{select} ORDER BY id DESC LIMIT ?2"),
            receiver,
            limit,
            source,
        )
    }

    /// As [`Log::stream`], for a query that states its own ordering.
    fn ordered(&self, sql: &str, receiver: i64, limit: i64, source: Source) -> Result<Vec<Note>> {
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
            .query_map((receiver, limit), |row| {
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
    pub(crate) fn read(&self, receiver: i64, window: Window, columns: usize) -> Result<History> {
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
               -- One unit per graph: two receivers' readings drawn
               -- together make a swap look like an oscillator moving.
               AND receiver_id = ?3
             GROUP BY bucket
             ORDER BY ago",
        )?;

        let mut out = History::default();
        let rows = statement.query_map((span, buckets, receiver), |row| {
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

#[cfg(test)]
mod tests {
    const PREFIX: &str = "smartclockmon";

    /// A database path that deletes itself, and the `-wal` and `-shm`
    /// SQLite writes beside it, on drop.  Drop also runs on a panicking
    /// test, where a line at the end of the test body would not.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "{}-{name}-{}.sqlite",
                PREFIX,
                std::process::id()
            ));
            let guard = Self(path);
            guard.wipe();
            guard
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }

        fn wipe(&self) {
            for suffix in ["", "-wal", "-shm"] {
                let mut name = self.0.clone().into_os_string();
                name.push(suffix);
                let _ = std::fs::remove_file(std::path::PathBuf::from(name));
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            self.wipe();
        }
    }

    use super::Log;
    use super::Source;
    use rusqlite::Connection;

    /// A log holding two receivers, with a cleared diagnostic log and
    /// a power-on stamped at midnight on a stale date.
    fn two_units(name: &str) -> Scratch {
        let scratch = Scratch::new(name);
        let path = scratch.path();
        let conn = Connection::open(path).expect("make a log");
        conn.execute_batch(
            r#"
            CREATE TABLE receiver (
                id INTEGER PRIMARY KEY, serial TEXT UNIQUE NOT NULL,
                manufacturer TEXT, model TEXT, firmware TEXT,
                first_seen TEXT, last_seen TEXT);
            INSERT INTO receiver VALUES
                (1,'AAA','HEWLETT-PACKARD','58503A','3704-C','x','2026-09-01T02:00:00Z'),
                (2,'BBB','HEWLETT-PACKARD','Z3805A','3611-A','x','2026-09-02T02:00:00Z');

            CREATE TABLE receiver_log (
                id INTEGER PRIMARY KEY, at TEXT NOT NULL, entry INTEGER NOT NULL,
                stamp TEXT, message TEXT NOT NULL, receiver_id INTEGER,
                generation INTEGER NOT NULL DEFAULT 0);
            INSERT INTO receiver_log (at, entry, stamp, message, receiver_id, generation) VALUES
                ('2026-09-01T00:00:00Z',1,'20050528.00:01:00','one A',         1,0),
                ('2026-09-01T00:00:01Z',2,'20050528.00:00:00','Power on',      1,0),
                ('2026-09-01T00:00:02Z',1,'20050530.00:00:00','after clear A', 1,1),
                ('2026-09-02T00:00:00Z',1,'20050601.00:00:00','one B',         2,0);

            CREATE TABLE receiver_event (
                id INTEGER PRIMARY KEY, at TEXT NOT NULL, register TEXT,
                bits INTEGER, decoded TEXT, receiver_id INTEGER);
            INSERT INTO receiver_event (at, register, bits, decoded, receiver_id) VALUES
                ('2026-09-01T00:00:00Z','alarm',0,'clear A',1),
                ('2026-09-02T00:00:00Z','alarm',8,'holdover B',2);

            CREATE TABLE receiver_error (
                id INTEGER PRIMARY KEY, at TEXT NOT NULL, code INTEGER,
                message TEXT, receiver_id INTEGER);
            INSERT INTO receiver_error (at, code, message, receiver_id) VALUES
                ('2026-09-01T00:00:00Z',-113,'undefined header A',1),
                ('2026-09-02T00:00:00Z',-230,'stale B',2);
            "#,
        )
        .expect("fill it");
        drop(conn);
        scratch
    }

    #[test]
    fn the_journal_holds_one_receiver_and_not_the_other() {
        let scratch = two_units("journal");
        let path = scratch.path();
        let log = Log::open(path).expect("open");
        let a = log.journal(1, 50).expect("A");
        let texts: Vec<&str> = a.iter().map(|n| n.text.as_str()).collect();
        assert!(
            texts.iter().all(|t| !t.contains('B')),
            "B's records must not appear in A's journal: {texts:?}"
        );
        assert!(texts.contains(&"clear A"));
        assert!(texts.contains(&"-113 undefined header A"));
    }

    #[test]
    fn a_power_on_does_not_sort_to_the_start_of_its_day() {
        // Before the first GPS lock the receiver stamps entries with
        // elapsed time since boot on a stale date, so ordering by the
        // stamp put `Power on` -- at 00:00:00 -- above the entry that
        // preceded it.  The generation and the entry number are the
        // receiver's own sequence and do not have that problem.
        let scratch = two_units("order");
        let path = scratch.path();
        let log = Log::open(path).expect("open");
        let journal = log.journal(1, 50).expect("A");
        let entries: Vec<&str> = journal
            .iter()
            .filter(|n| n.source == Source::Log)
            .map(|n| n.text.as_str())
            .collect();
        assert_eq!(entries, vec!["after clear A", "Power on", "one A"]);
    }

    #[test]
    fn the_receivers_are_listed_most_recently_seen_first() {
        let scratch = two_units("list");
        let path = scratch.path();
        let log = Log::open(path).expect("open");
        let found = log.receivers().expect("list");
        assert_eq!(
            found.iter().map(super::Receiver::label).collect::<Vec<_>>(),
            vec!["Z3805A BBB".to_owned(), "58503A AAA".to_owned()]
        );
    }
}
