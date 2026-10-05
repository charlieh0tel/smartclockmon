//! Reading the log.
//!
//! Opened read-only: the daemon is the only writer, and a reader that
//! cannot write cannot corrupt a record that exists to be trusted.  The
//! daemon runs the file in WAL, so a reader runs alongside it without
//! coordination and without holding it up.
//!
//! Bucketed and thinned in SQL rather than by the caller.  A week at
//! the fast tier is about six hundred thousand rows, and carrying those
//! across the process boundary only to throw most away would be slow in
//! exactly the places -- a phone, a terminal redrawing -- that have no
//! cycles to spare.
//!
//! A reader is not a writer and must not insist the file be as new as
//! itself.  A table or column this log predates reads as empty rather
//! than as an error, so a viewer upgraded before the daemon restarts,
//! or pointed at an archived log, shows what is there.

use std::path::Path;

use rusqlite::Connection;
use rusqlite::OpenFlags;
use rusqlite::Row;
use serde::Serialize;
use smartclock::adev::Curve;
use smartclock::adev::MAX_SAMPLES;
use smartclock::snapshot::Tier;
use smartclock::task::Cadence;

use crate::error::Error;
use crate::error::Result;
use crate::schema::PLOTTABLE;
use crate::schema::cadence_key;
use crate::schema::has_column;

/// The most rows one Allan deviation reads, after held readings are
/// thinned out.
///
/// Twice what the estimator grids before it starts striding: about two
/// months of ten-second updates, and some tens of megabytes held at
/// once.  A longer range is measured over its newest rows and says so,
/// rather than reading a year of the log into memory.
pub const MAX_PHASE_ROWS: usize = 2 * MAX_SAMPLES;

/// The most series one request will bucket together.
///
/// Enough to ask for every plottable column at once.  Each adds three
/// aggregates to the same query, which is cheap.
const MAX_SERIES: usize = PLOTTABLE.len();

/// Fewest buckets worth drawing, and the most a chart can show.
///
/// The upper bound is about three times the pixels across a wide
/// screen, so asking for more cannot make the picture better and can
/// make the query slow.
const MIN_POINTS: usize = 16;
const MAX_POINTS: usize = 5000;

/// How many empty buckets in a row make a gap worth breaking a line at.
///
/// A bucket nobody wrote in produces no row, and a chart drawn from the
/// rows alone joins the points either side of the hole with a straight
/// line -- which reads as a receiver sitting perfectly steady for the
/// hours it was in fact unplugged.  A null in the gap makes the line
/// break instead.
///
/// Only for a hole several buckets wide, though.  The medium tier holds
/// the line for about three seconds and the fast tier cannot run
/// meanwhile, so at an hour's zoom -- buckets of about two seconds --
/// every one of those stalls empties a bucket, and breaking on each
/// drew every trace as a dashed line at the daemon's own polling
/// rhythm.  The threshold scales with the view: a few buckets is
/// seconds at an hour and hours at a month, which is the right shape,
/// because what counts as a gap is relative to what is being looked at.
const GAP_BUCKETS: i64 = 4;

/// The most of each journal stream one read returns.
const MAX_JOURNAL: usize = 500;

/// Rows the fast tier measured, rather than rows restating its last
/// reading.
///
/// The slower tiers publish rows of their own carrying the fast tier's
/// values again.  `fast_at` says exactly which rows measured; the
/// freshness flag does not, since it reports the whole snapshot and
/// goes stale when some other tier fails.  Rows from before `fast_at`
/// existed fall back to the flag.
const MEASURED: &str = "(fast_at = at OR (fast_at IS NULL AND freshness = 'live'))";

/// A bound of a range, `seconds` an SQL expression in unix seconds, as
/// text comparable with `at`.
///
/// Compared as text, against the column itself, so the index on `at`
/// can be used; `unixepoch(at) >= ?` reads every row in the table.  The
/// stored form is RFC 3339 with fractional seconds and a Z, so a bound
/// truncated to the second sorts before every row within that second,
/// which is what an inclusive lower and an exclusive upper bound want.
fn text_bound(seconds: &str) -> String {
    format!("strftime('%Y-%m-%dT%H:%M:%S', {seconds}, 'unixepoch')")
}

/// A snapshot column as an SQL expression that is null where its value
/// is not current.
///
/// A medium or slow column rides in every fast row with whatever its
/// tier last read, so it counts only while its tier's timestamp is
/// within [`Cadence::current_window`] of the row.  `column` is
/// interpolated and must be a known column name, never caller input.
/// Rows written before the tier timestamps existed have no `fast_at`
/// and are taken as they are.
fn current(column: &str, tier: Tier, cadence: &Cadence) -> String {
    if tier == Tier::Fast {
        return column.to_owned();
    }
    let window = cadence.current_window(tier);
    let read = tier.name();
    format!(
        "CASE WHEN fast_at IS NULL
                OR unixepoch({read}_at, 'subsec') >= unixepoch(at, 'subsec') - {window}
              THEN {column} END"
    )
}

/// The daemon's log, open for reading.
#[derive(Debug)]
pub struct Log {
    conn: Connection,
}

/// One receiver the log holds rows for.
#[derive(Debug, Clone, Serialize)]
pub struct Receiver {
    /// The `receiver_id` every row of this unit's carries.  Its own
    /// log's numbering, so not published: a unit is named by serial.
    #[serde(skip)]
    pub id: i64,
    /// What the unit calls itself, and the only field it is known by:
    /// a firmware upgrade must not make it a different receiver.
    pub serial: String,
    /// Its model, empty where the identity did not parse.
    pub model: String,
    /// Its firmware, as last seen.
    pub firmware: String,
    /// When the log first recorded it.
    pub first_seen: String,
    /// When the log last recorded a measurement from it.
    pub last_seen: String,
    /// The internal GPS engine's identity as the receiver answered it,
    /// once a daemon has read it.
    pub gps_engine: Option<String>,
}

impl Receiver {
    /// How a status line names it.
    pub fn label(&self) -> String {
        if self.model.is_empty() {
            self.serial.clone()
        } else {
            format!("{} {}", self.model, self.serial)
        }
    }
}

/// Bucketed readings of several columns over one range.
#[derive(Debug, Serialize)]
pub struct Series {
    /// The bucket centres in unix seconds, shared by every plot below.
    ///
    /// One array, not one per plot: it is what makes stacked charts
    /// line up, and sending it once says so.
    pub at: Vec<f64>,
    /// One per column asked for, in the order asked.
    pub plots: Vec<Plot>,
}

/// One column's buckets, against [`Series::at`].
///
/// `min` and `max` are kept because a mean alone hides the thing worth
/// seeing.  A step that lasted a second inside a ten minute bucket
/// moves the mean imperceptibly and the max completely.  Each is null
/// in a bucket with no current reading, and in the bucket that marks a
/// gap.
#[derive(Debug, Serialize)]
pub struct Plot {
    /// Which column this is.
    pub column: String,
    /// Mean of each bucket.
    pub mean: Vec<Option<f64>>,
    /// Lowest reading in each bucket.
    pub min: Vec<Option<f64>>,
    /// Highest reading in each bucket.
    pub max: Vec<Option<f64>>,
}

/// What the receiver has recorded about itself, each record newest
/// first and limited on its own.
#[derive(Debug, Default, Serialize)]
pub struct Journal {
    /// Its diagnostic log, as copied out.
    pub entries: Vec<Entry>,
    /// Transitions taken from its event registers.
    pub events: Vec<Event>,
    /// Entries taken from its error queue.
    pub errors: Vec<ReceiverError>,
    /// What a person wrote about it, by when it happened.
    pub notes: Vec<Note>,
}

/// One diagnostic log entry.
#[derive(Debug, Serialize)]
pub struct Entry {
    /// When it was copied out.
    pub at: String,
    /// The receiver's own entry number.
    pub entry: i64,
    /// Its own timestamp, as written -- on the receiver's calendar,
    /// which may be behind by whole GPS epochs.  Empty where it had
    /// none.
    pub stamp: String,
    /// What it says.
    pub message: String,
}

/// One latched transition.
#[derive(Debug, Serialize)]
pub struct Event {
    /// When it was read, which is within one journal pass of when it
    /// happened.
    pub at: String,
    /// Which register it came from.
    pub register: String,
    /// The raw word.
    pub bits: i64,
    /// Its bits named.
    pub decoded: String,
}

/// One entry from the error queue.
#[derive(Debug, Serialize)]
pub struct ReceiverError {
    /// When it was read.  The queue carries no timestamps of its own.
    pub at: String,
    /// SCPI error code.
    pub code: i64,
    /// What the receiver called it.
    pub message: String,
}

/// A person's note about the receiver or the bench.
#[derive(Debug, Serialize)]
pub struct Note {
    /// When it happened, as the writer gave it.
    pub at: String,
    /// What it says.
    pub text: String,
}

/// A fact about the unit, as a person recorded it.
#[derive(Debug, Serialize)]
pub struct Fact {
    /// When the value became true.
    pub since: String,
    /// What it is about, such as `ocxo.serial`.
    pub key: String,
    /// Its value.
    pub value: String,
}

/// `path` as an SQLite URI that opens it immutable.  `?`, `#` and `%`
/// would be read as URI syntax, so they are escaped.
fn immutable_uri(path: &str) -> String {
    let escaped: String = path
        .chars()
        .map(|c| match c {
            '?' => "%3F".to_owned(),
            '#' => "%23".to_owned(),
            '%' => "%25".to_owned(),
            other => other.to_string(),
        })
        .collect();
    format!("file:{escaped}?immutable=1")
}

/// Whether a failure is a table this log predates.
fn missing_table(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(_, Some(why)) if why.contains("no such table"))
}

impl Log {
    /// Open the log without taking a write lock on it.
    ///
    /// A WAL log whose `-shm` is gone, in a directory this reader may not
    /// write, cannot be read normally: SQLite would have to create the
    /// file.  Such a log has no writer -- a daemon holding it keeps
    /// `-shm` -- so it is opened immutable instead, which reads the file
    /// as it stands.
    pub fn open(path: &Path) -> Result<Self> {
        let opened = |target: &Path| {
            Connection::open_with_flags(
                target,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
            )
            .map_err(|source| Error::Open {
                path: path.to_path_buf(),
                source,
            })
        };
        let conn = opened(path)?;
        let unreadable = matches!(
            conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(())),
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ReadOnly
                    || e.code == rusqlite::ErrorCode::CannotOpen
        );
        match path.to_str() {
            Some(name) if unreadable => {
                drop(conn);
                Ok(Self {
                    conn: opened(Path::new(&immutable_uri(name)))?,
                })
            }
            _ => Ok(Self { conn }),
        }
    }

    /// Every receiver this log holds, most recently seen first.
    ///
    /// Empty on a log written before receivers were recorded, which is
    /// not an error: such a log has one unit's rows and no name for it.
    pub fn receivers(&self) -> Result<Vec<Receiver>> {
        // The engine's identity arrived with schema 9; a log no daemon
        // of that version has opened lacks the column.
        let engine = if has_column(&self.conn, "receiver", "gps_engine")? {
            "gps_engine"
        } else {
            "NULL"
        };
        let mut statement = match self.conn.prepare(&format!(
            "SELECT id, serial, COALESCE(model, ''), COALESCE(firmware, ''),
                    COALESCE(first_seen, ''), COALESCE(last_seen, ''), {engine}
             FROM receiver ORDER BY last_seen DESC"
        )) {
            Ok(statement) => statement,
            Err(e) if missing_table(&e) => return Ok(Vec::new()),
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
                    gps_engine: row.get(6)?,
                })
            })?
            .collect::<std::result::Result<_, _>>()?)
    }

    /// How often the daemon ran each tier, as it recorded.
    ///
    /// The default for any tier not recorded: a log written before the
    /// cadence was recorded ran at the defaults of its day or at
    /// whatever flags it was given, and the defaults are the likelier.
    pub fn cadence(&self) -> Cadence {
        let default = Cadence::default();
        let recorded = |tier: Tier| {
            let value: String = self
                .conn
                .query_row(
                    "SELECT value FROM meta WHERE key = ?1",
                    [cadence_key(tier)],
                    |row| row.get(0),
                )
                .ok()?;
            std::time::Duration::try_from_secs_f64(value.parse().ok()?).ok()
        };
        Cadence {
            fast: recorded(Tier::Fast).unwrap_or(default.fast),
            medium: recorded(Tier::Medium).unwrap_or(default.medium),
            slow: recorded(Tier::Slow).unwrap_or(default.slow),
        }
    }

    /// Bucketed series for several columns at once, between two unix
    /// times and into at most `points` buckets each.
    ///
    /// One query rather than one per column, and that is a correctness
    /// requirement rather than an optimization.  Stacked charts are
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
    /// Only rows the fast tier measured are counted, and a slower
    /// tier's column only while that tier's value is current.
    pub fn series(
        &self,
        receiver: i64,
        columns: &[String],
        from: i64,
        to: i64,
        points: usize,
    ) -> Result<Series> {
        if columns.is_empty() {
            return Err(Error::Request("no columns asked for".to_owned()));
        }
        if columns.len() > MAX_SERIES {
            return Err(Error::Request(format!(
                "at most {MAX_SERIES} series at once"
            )));
        }
        let tiers = columns
            .iter()
            .map(|column| {
                PLOTTABLE
                    .iter()
                    .find(|(name, _, _)| name == column)
                    .map(|&(_, tier, _)| tier)
                    .ok_or_else(|| Error::Request(format!("{column} is not a column this serves")))
            })
            .collect::<Result<Vec<Tier>>>()?;
        if from > to {
            return Err(Error::Request("the range ends before it starts".to_owned()));
        }
        let cadence = self.cadence();
        let points = points.clamp(MIN_POINTS, MAX_POINTS) as i64;
        let span = to.saturating_sub(from).max(1);
        let aggregates = columns
            .iter()
            .zip(&tiers)
            .map(|(c, &tier)| {
                let value = current(c, tier, &cadence);
                format!("AVG({value}), MIN({value}), MAX({value})")
            })
            .collect::<Vec<_>>()
            .join(", ");
        let (lower, upper) = (text_bound("?1"), text_bound("?2 + 1"));
        let sql = format!(
            // Divided by the span plus one so that a row landing exactly
            // on `to` falls in the last bucket rather than in one past
            // it: the inclusive range would otherwise return points + 1
            // buckets, which is not what the caller asked for.
            "SELECT CAST((unixepoch(at) - ?1) * ?3 / (?4 + 1) AS INTEGER) AS bucket,
                    AVG(unixepoch(at)) AS at,
                    {aggregates}
             FROM snapshot
             WHERE at >= {lower}
               AND at < {upper}
               AND {MEASURED}
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
        let mut previous: Option<i64> = None;
        let bucket_width = (span as f64 + 1.0) / points as f64;
        while let Some(row) = rows.next()? {
            let bucket: i64 = row.get(0)?;
            let when: f64 = row.get(1)?;
            if previous.is_some_and(|last| bucket > last + GAP_BUCKETS) {
                at.push(when - bucket_width);
                for plot in &mut plots {
                    plot.mean.push(None);
                    plot.min.push(None);
                    plot.max.push(None);
                }
            }
            previous = Some(bucket);
            at.push(when);
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

    /// The phase readings for an Allan deviation, with the run already
    /// divided where it must not be joined.
    ///
    /// The readings are taken at full rate rather than bucketed: a
    /// deviation is not a time series, and averaging readings before
    /// the estimator sees them is the operation it exists to perform.
    /// A second difference taken across a relock, a holdover or a power
    /// cycle is not a measurement of the oscillator, so those become
    /// segment boundaries and no triple is formed across one.  The mode
    /// and the holdover flag are what the log records about that; a
    /// plain gap in the timestamps is caught by the estimator itself.
    ///
    /// Also whether the range held more than `limit` changes, in which
    /// case the curve is of the newest that many.
    pub fn phase(&self, receiver: i64, from: i64, to: i64, limit: usize) -> Result<(Curve, bool)> {
        if from > to {
            return Err(Error::Request("the range ends before it starts".to_owned()));
        }
        let (lower, upper) = (text_bound("?2"), text_bound("?3 + 1"));
        let mut statement = self.conn.prepare(&format!(
            // `at` as stored, not `unixepoch(at)`: that truncates to
            // the second, and the sub-second part is what decides which
            // grid point a reading belongs to.
            //
            // A row is kept only where the interval or the state differs
            // from the row before.  The receiver updates the interval
            // every ten seconds and is polled every second, so this is
            // the same thinning `Curve::from_readings` does, done where
            // it saves reading nine rows in ten into memory; the state
            // changes are kept because the run must break at them.
            // Newest first, so the limit keeps the end of the range.
            "SELECT at, time_interval_s, mode, holdover FROM (
                 SELECT at, time_interval_s,
                        COALESCE(mode, 'unknown') AS mode,
                        COALESCE(holdover_active, 0) AS holdover,
                        LAG(time_interval_s) OVER previous AS was_interval,
                        LAG(COALESCE(mode, 'unknown')) OVER previous AS was_mode,
                        LAG(COALESCE(holdover_active, 0)) OVER previous AS was_holdover
                 FROM snapshot
                 WHERE at >= {lower}
                   AND at < {upper}
                   AND receiver_id = ?1
                   AND {MEASURED}
                 WINDOW previous AS (ORDER BY at))
             -- IS NOT rather than <>, so a missing interval counts as
             -- different from a present one: a row without one still
             -- carries the state, and a run of them is kept by its
             -- first row.
             WHERE was_mode IS NULL
                OR time_interval_s IS NOT was_interval
                OR mode <> was_mode
                OR holdover <> was_holdover
             ORDER BY at DESC
             LIMIT ?4"
        ))?;
        // One more than the limit, to know whether there were more.
        let asked = i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX);
        let rows = statement.query_map(rusqlite::params![receiver, from, to, asked], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<f64>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)? != 0,
            ))
        })?;

        // Rows without an interval are kept for their state; see
        // `Curve::from_logged`.
        let mut logged = Vec::new();
        for row in rows {
            let (at, interval, mode, holdover) = row?;
            let Ok(at) = at.parse::<jiff::Timestamp>() else {
                continue;
            };
            logged.push((at, interval, (mode, holdover)));
        }
        let truncated = logged.len() > limit;
        logged.truncate(limit);
        logged.reverse();

        Ok((Curve::from_logged(logged), truncated))
    }

    /// The oldest and newest readings, in unix seconds, so a view can
    /// offer a range that exists rather than one that might not.
    pub fn extent(&self, receiver: i64) -> Result<(f64, f64)> {
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

    /// The receiver's own record-keeping, at most `limit` of each.
    ///
    /// Each stream limited on its own rather than together: the
    /// diagnostic log is the receiver's history and the other two are
    /// what happened recently, and one must not crowd out the other.
    pub fn journal(&self, receiver: i64, limit: usize) -> Result<Journal> {
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
            notes: self.stream(
                receiver,
                "SELECT at, text FROM note
                 WHERE receiver_id = ?1 ORDER BY at DESC, id DESC",
                limit,
                |row| {
                    Ok(Note {
                        at: row.get(0)?,
                        text: row.get(1)?,
                    })
                },
            )?,
        })
    }

    /// The current value of each fact about the receiver, by key: the
    /// one with the latest `since`, an older value having been
    /// replaced.
    pub fn facts(&self, receiver: i64) -> Result<Vec<Fact>> {
        self.stream(
            receiver,
            "SELECT since, key, value FROM fact AS f
             WHERE receiver_id = ?1 AND id = (
                 SELECT id FROM fact
                 WHERE receiver_id = f.receiver_id AND key = f.key
                 ORDER BY since DESC, id DESC LIMIT 1)
             ORDER BY key",
            MAX_JOURNAL as i64,
            |row| {
                Ok(Fact {
                    since: row.get(0)?,
                    key: row.get(1)?,
                    value: row.get(2)?,
                })
            },
        )
    }

    /// One journal stream, empty if this log predates it.
    fn stream<T, F>(&self, receiver: i64, select: &str, limit: i64, read: F) -> Result<Vec<T>>
    where
        F: Fn(&Row<'_>) -> rusqlite::Result<T>,
    {
        let sql = format!("{select} LIMIT ?2");
        let mut statement = match self.conn.prepare(&sql) {
            Ok(statement) => statement,
            Err(e) if missing_table(&e) => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        Ok(statement
            .query_map((receiver, limit), |row| read(row))?
            .collect::<std::result::Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests;
