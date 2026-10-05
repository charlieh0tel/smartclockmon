//! The daemon's log, shaped for the monitor's panes.
//!
//! The reading itself is `smartclock_log`'s, shared with the web view;
//! what is here is what a terminal wants from it: windows that end
//! now, time as seconds ago, and the journal's three records as one
//! list.

use std::path::Path;

use anyhow::Result;
use smartclock::adev::Curve;
use smartclock_log::reader;
use smartclock_log::reader::MAX_PHASE_ROWS;
use smartclock_log::reader::Receiver;

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
    pub(crate) fn seconds(self) -> i64 {
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
    /// Where each unbroken run of columns starts, as indices into the
    /// series above.
    ///
    /// A column with no current reading -- the receiver unplugged, the
    /// daemon stopped, a slower tier failing -- ends a run, so a chart
    /// breaks the line there instead of drawing it straight across as
    /// though the value had held.
    pub(crate) runs: Vec<usize>,
    /// Whether a column with no reading has come since the last one
    /// pushed.
    broken: bool,
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

    /// Each unbroken run's mean, lowest and highest.
    pub(crate) fn each_run(&self) -> impl Iterator<Item = [&[(f64, f64)]; 3]> {
        self.runs.iter().enumerate().map(|(n, &start)| {
            let end = self.runs.get(n + 1).copied().unwrap_or(self.mean.len());
            [
                &self.mean[start..end],
                &self.low[start..end],
                &self.high[start..end],
            ]
        })
    }

    fn push(&mut self, ago: f64, mean: Option<f64>, low: Option<f64>, high: Option<f64>) {
        let (Some(mean), Some(low), Some(high)) = (mean, low, high) else {
            self.broken = true;
            return;
        };
        if self.broken || self.runs.is_empty() {
            self.runs.push(self.mean.len());
            self.broken = false;
        }
        self.mean.push((ago, mean));
        self.low.push((ago, low));
        self.high.push((ago, high));
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
}

/// Which of the receiver's records a line came from.
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
    /// A short tag for the column that says where a line came from.
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
pub(crate) struct Line {
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

/// The columns the graphs draw, in the order [`History`] holds them.
const GRAPHED: [&str; 3] = ["efc_percent", "temperature_c", "time_interval_s"];

/// The daemon's log, open for reading.
#[derive(Debug)]
pub(crate) struct Log(reader::Log);

impl Log {
    /// Open the log without taking a write lock on it.
    pub(crate) fn open(path: &Path) -> Result<Self> {
        Ok(Self(reader::Log::open(path)?))
    }

    /// Every receiver this log holds, most recently seen first.
    pub(crate) fn receivers(&self) -> Result<Vec<Receiver>> {
        Ok(self.0.receivers()?)
    }

    /// The receiver's own record-keeping, at most `limit` of each
    /// record.
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
    pub(crate) fn journal(&self, receiver: i64, limit: usize) -> Result<Vec<Line>> {
        let journal = self.0.journal(receiver, limit)?;
        // Events and errors share the host clock, so these two do
        // merge, newest first.
        let mut lines: Vec<Line> = journal
            .events
            .into_iter()
            .map(|event| Line {
                stamp: event.at.clone(),
                at: event.at,
                text: event.decoded,
                source: Source::Event,
            })
            .chain(journal.errors.into_iter().map(|error| Line {
                stamp: error.at.clone(),
                at: error.at,
                text: format!("{} {}", error.code, error.message),
                source: Source::Error,
            }))
            .collect();
        lines.sort_by(|a, b| b.at.cmp(&a.at));
        // Then the receiver's log in the receiver's own sequence.  The
        // receiver's own stamp where there is one: an entry it
        // timestamped itself is better dated by the receiver than by
        // when we happened to copy it out, even though that clock can
        // be behind by whole GPS epochs.
        lines.extend(journal.entries.into_iter().map(|entry| Line {
            stamp: if entry.stamp.is_empty() {
                entry.at.clone()
            } else {
                entry.stamp
            },
            at: entry.at,
            text: entry.message,
            source: Source::Log,
        }));
        Ok(lines)
    }

    /// The Allan deviation of the 1 PPS interval over the window.
    pub(crate) fn deviation(&self, receiver: i64, window: Window) -> Result<Curve> {
        let now = jiff::Timestamp::now().as_second();
        let (curve, _) = self
            .0
            .phase(receiver, now - window.seconds(), now, MAX_PHASE_ROWS)?;
        Ok(curve)
    }

    /// Read the series a graph needs, thinned to at most `columns`
    /// points, since a week at one second is six hundred thousand rows
    /// and a terminal has a couple of hundred columns.
    pub(crate) fn read(&self, receiver: i64, window: Window, columns: usize) -> Result<History> {
        let now = jiff::Timestamp::now().as_second();
        let asked = GRAPHED.map(str::to_owned);
        let series = self
            .0
            .series(receiver, &asked, now - window.seconds(), now, columns)?;
        let mut traces = [Trace::default(), Trace::default(), Trace::default()];
        for (trace, plot) in traces.iter_mut().zip(&series.plots) {
            for (n, &at) in series.at.iter().enumerate() {
                trace.push(at - now as f64, plot.mean[n], plot.min[n], plot.max[n]);
            }
        }
        let [efc, temperature, mut time_interval] = traces;
        // Nanoseconds: the interval is a few parts in a billion of a
        // second and would render as a flat zero.
        time_interval.scale(1e9);
        Ok(History {
            efc,
            temperature,
            time_interval,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Line;
    use super::Log;
    use super::Series;
    use super::Source;
    use super::Window;
    use rusqlite::Connection;
    use smartclock_log::schema::META;
    use smartclock_log::schema::TABLES;
    use smartclock_log::schema::stored;

    /// A log path that deletes itself, and the `-wal` and `-shm` SQLite
    /// writes beside it, on drop.  Drop also runs on a panicking test.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        /// A new log holding the empty tables and one receiver.
        fn new(name: &str) -> Self {
            let guard = Self(std::env::temp_dir().join(format!(
                "smartclockmon-{name}-{}.sqlite",
                std::process::id()
            )));
            guard.wipe();
            let conn = guard.connect();
            conn.execute_batch(META).expect("meta");
            conn.execute_batch(TABLES).expect("the tables");
            conn.execute_batch(
                "INSERT INTO receiver (id, serial, first_seen, last_seen)
                 VALUES (1, 'AAA', '', '');",
            )
            .expect("a receiver");
            guard
        }

        fn connect(&self) -> Connection {
            Connection::open(&self.0).expect("open the log for writing")
        }

        fn open(&self) -> Log {
            Log::open(&self.0).expect("open the log for reading")
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

    #[test]
    fn a_column_with_no_reading_breaks_the_trace_into_runs() {
        let mut trace = super::Trace::default();
        trace.push(-30.0, None, None, None);
        trace.push(-20.0, Some(1.0), Some(1.0), Some(1.0));
        trace.push(-15.0, Some(2.0), Some(2.0), Some(2.0));
        trace.push(-10.0, None, None, None);
        trace.push(-5.0, Some(3.0), Some(3.0), Some(3.0));
        let means: Vec<Vec<f64>> = trace
            .each_run()
            .map(|[mean, _, _]| mean.iter().map(|&(_, v)| v).collect())
            .collect();
        assert_eq!(means, vec![vec![1.0, 2.0], vec![3.0]]);
    }

    #[test]
    fn temperature_is_not_drawn_after_its_tier_stops_reading() {
        // Two minutes of fast rows ending now.  The medium tier reads
        // each second for the first ten and then fails, so its last
        // read is 110 s ago and its value is current for three of its
        // ten-second intervals after that: to 80 s ago.
        let scratch = Scratch::new("stale-tier");
        let conn = scratch.connect();
        conn.execute_batch("INSERT INTO meta VALUES ('cadence_medium', '10');")
            .expect("cadence");
        let now = jiff::Timestamp::now().as_second();
        let at = |second: i64| stored(jiff::Timestamp::from_second(second).expect("a timestamp"));
        for ago in (0..120).rev() {
            conn.execute(
                "INSERT INTO snapshot
                 (at, freshness, fast_at, medium_at, efc_percent, temperature_c, receiver_id)
                 VALUES (?1, 'stale', ?1, ?2, 50.0, 35.0, 1)",
                rusqlite::params![at(now - ago), at(now - ago.max(110))],
            )
            .expect("a row");
        }
        drop(conn);

        let history = scratch.open().read(1, Window::Hour, 1024).expect("read");
        let newest = |series: &Series| {
            series
                .iter()
                .map(|&(ago, _)| ago)
                .fold(f64::NEG_INFINITY, f64::max)
        };
        assert!(newest(&history.efc.mean) > -5.0);
        let temperature = newest(&history.temperature.mean);
        assert!((-85.0..=-75.0).contains(&temperature), "{temperature}");
    }

    #[test]
    fn recent_records_come_first_and_the_receivers_log_after_in_its_own_order() {
        let scratch = Scratch::new("journal");
        scratch
            .connect()
            .execute_batch(
                "INSERT INTO receiver_log (at, entry, stamp, message, receiver_id, generation) VALUES
                     ('2026-09-01T00:00:00.000000000Z', 1, '20050528.00:01:00', 'one', 1, 0),
                     ('2026-09-01T00:00:01.000000000Z', 2, NULL, 'two', 1, 0);
                 INSERT INTO receiver_event (at, register, bits, decoded, receiver_id) VALUES
                     ('2026-09-01T00:00:00.000000000Z', 'alarm', 0, 'clear', 1);
                 INSERT INTO receiver_error (at, code, message, receiver_id) VALUES
                     ('2026-09-01T00:00:05.000000000Z', -113, 'undefined header', 1);",
            )
            .expect("fill it");
        let lines: Vec<Line> = scratch.open().journal(1, 50).expect("journal");
        assert_eq!(
            lines
                .iter()
                .map(|n| (n.text.as_str(), n.source))
                .collect::<Vec<_>>(),
            vec![
                ("-113 undefined header", Source::Error),
                ("clear", Source::Event),
                ("two", Source::Log),
                ("one", Source::Log),
            ]
        );
        // An entry the receiver did not stamp is dated by when it was
        // read; one it did keeps its own stamp.
        assert_eq!(lines[2].stamp, "2026-09-01T00:00:01.000000000Z");
        assert_eq!(lines[3].stamp, "20050528.00:01:00");
    }
}
