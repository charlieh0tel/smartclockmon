//! The sensor service's log: its tables, and reading them.
//!
//! A log of its own, apart from the receivers', because a sensor
//! belongs to the host: it reads whether or not any receiver is
//! attached.  The service writes it; the web view and the exporter
//! read it, from the same definitions, as the receivers' logs are read.

use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;
use serde::Serialize;
use smartclock::sensors::DEFAULT_EVERY_S;
use smartclock::sensors::PERIODS_STALE;

use crate::error::Result;
use crate::reader::GAP_BUCKETS;
use crate::reader::MAX_POINTS;
use crate::reader::MIN_POINTS;
use crate::reader::missing_table;
use crate::reader::open_read_only;
use crate::reader::text_bound;

/// Bumped when the tables change shape, or what a column holds.  The
/// service refuses a log of a later version; the readers accept any.
pub const VERSION: i64 = 1;

/// The tables, as of [`VERSION`], each `IF NOT EXISTS`.
pub const TABLES: &str = r#"
    -- One row per sensor: a name and what it measures, together, so a
    -- part reading two quantities is one name twice.
    CREATE TABLE IF NOT EXISTS sensor (
        id       INTEGER PRIMARY KEY,
        name     TEXT NOT NULL,
        quantity TEXT NOT NULL,
        unit     TEXT NOT NULL,
        UNIQUE (name, quantity)
    );

    -- Where a sensor was read from, from when: the path as configured,
    -- and the kernel's name for the device behind it.  A new row
    -- whenever either changes, so an old reading keeps the source it
    -- was read from.
    CREATE TABLE IF NOT EXISTS source (
        sensor_id INTEGER NOT NULL REFERENCES sensor(id),
        since     TEXT NOT NULL,
        source    TEXT NOT NULL,
        device    TEXT
    );
    CREATE INDEX IF NOT EXISTS source_sensor ON source(sensor_id, since);

    -- How often the service read its sensors, from when: a new row
    -- whenever it is started with another period, so older readings
    -- are judged by the period they were read at.
    CREATE TABLE IF NOT EXISTS period (
        since TEXT NOT NULL,
        every REAL NOT NULL
    );

    -- One row per reading that succeeded.  A read that failed writes
    -- nothing: a gap in the readings is the record of it.
    CREATE TABLE IF NOT EXISTS reading (
        sensor_id INTEGER NOT NULL REFERENCES sensor(id),
        at        TEXT NOT NULL,
        value     REAL NOT NULL,
        PRIMARY KEY (sensor_id, at)
    ) WITHOUT ROWID;
"#;

/// The sensor service's log, open for reading.
#[derive(Debug)]
pub struct SensorLog {
    conn: Connection,
}

/// One sensor the log holds.
#[derive(Debug, Clone, Serialize)]
pub struct Sensor {
    /// Its row, the log's own numbering.
    #[serde(skip)]
    pub id: i64,
    /// What it is called.
    pub name: String,
    /// What it measures: `temperature`, `humidity` or `pressure`.
    pub quantity: String,
    /// The unit its readings are in.
    pub unit: String,
    /// Where it was last read from, as configured.
    pub source: Option<String>,
    /// The kernel's name for the device behind that, when it was read.
    pub device: Option<String>,
}

/// One sensor's readings over a range, bucketed.
#[derive(Debug, Serialize)]
pub struct Line {
    /// Which sensor.
    pub name: String,
    /// Each bucket's mean time, in unix seconds.
    pub at: Vec<f64>,
    /// Each bucket's mean, null in the bucket that marks a gap.
    pub values: Vec<Option<f64>>,
}

impl SensorLog {
    /// Open the log without taking a write lock on it.
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            conn: open_read_only(path)?,
        })
    }

    /// Every sensor the log holds a reading of, with where it was last
    /// read from.  One configured but never read -- a path that named
    /// nothing -- is left out, since there is nothing of it to show.
    /// Empty for a log with no tables yet.
    pub fn sensors(&self) -> Result<Vec<Sensor>> {
        let mut statement = match self.conn.prepare(
            "SELECT s.id, s.name, s.quantity, s.unit,
                    (SELECT source FROM source WHERE sensor_id = s.id
                     ORDER BY since DESC LIMIT 1),
                    (SELECT device FROM source WHERE sensor_id = s.id
                     ORDER BY since DESC LIMIT 1)
             FROM sensor s
             WHERE EXISTS (SELECT 1 FROM reading WHERE sensor_id = s.id)
             ORDER BY s.quantity, s.name",
        ) {
            Ok(statement) => statement,
            Err(e) if missing_table(&e) => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        Ok(statement
            .query_map([], |row| {
                Ok(Sensor {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    quantity: row.get(2)?,
                    unit: row.get(3)?,
                    source: row.get(4)?,
                    device: row.get(5)?,
                })
            })?
            .collect::<std::result::Result<_, _>>()?)
    }

    /// The first and last reading's times in unix seconds, or `None`
    /// for a log with none.  Each sensor's through the reading table's
    /// key, rather than a scan of every reading, which on a year's log
    /// is long enough to hold up a page.
    pub fn extent(&self) -> Result<Option<(f64, f64)>> {
        let span = self.conn.query_row(
            "SELECT unixepoch(MIN(first), 'subsec'), unixepoch(MAX(last), 'subsec')
             FROM (SELECT (SELECT MIN(at) FROM reading WHERE sensor_id = s.id) AS first,
                          (SELECT MAX(at) FROM reading WHERE sensor_id = s.id) AS last
                   FROM sensor s)",
            [],
            |row| Ok((row.get::<_, Option<f64>>(0)?, row.get::<_, Option<f64>>(1)?)),
        );
        match span {
            Ok((Some(first), Some(last))) => Ok(Some((first, last))),
            Ok(_) => Ok(None),
            Err(e) if missing_table(&e) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// How often the service reads its sensors now, as it last
    /// recorded; `None` if it did not.
    pub fn every(&self) -> Option<Duration> {
        let every = self.periods().ok()?.last()?.1;
        Duration::try_from_secs_f64(every).ok()
    }

    /// Every read period recorded, oldest first: from when, in unix
    /// seconds, and the period in seconds.
    fn periods(&self) -> Result<Vec<(f64, f64)>> {
        let mut statement = match self
            .conn
            .prepare("SELECT unixepoch(since, 'subsec'), every FROM period ORDER BY since")
        {
            Ok(statement) => statement,
            Err(e) if missing_table(&e) => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        Ok(statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?)
    }

    /// Every sensor of `quantity`, its readings between two unix times
    /// bucketed into at most `points` buckets, on the grid the
    /// receivers' history uses for the same range.
    ///
    /// A run of empty buckets is a gap, and breaks the line, only when
    /// it is longer than [`PERIODS_STALE`] read periods, the period the
    /// reading before it was read at, which is when the next was due:
    /// at an hour's zoom a bucket is a few seconds, and every bucket
    /// between two readings ten seconds apart would otherwise break it.
    pub fn series(&self, quantity: &str, from: i64, to: i64, points: usize) -> Result<Vec<Line>> {
        let points = points.clamp(MIN_POINTS, MAX_POINTS) as i64;
        let span = to.saturating_sub(from).max(1);
        let width = (span as f64 + 1.0) / points as f64;
        let periods = self.periods()?;
        // The period a reading at `at` was read at: the latest recorded
        // by then, or the first for a reading before any.
        let every_at = |at: f64| {
            periods
                .iter()
                .rev()
                .find(|(since, _)| *since <= at)
                .or(periods.first())
                .map_or(DEFAULT_EVERY_S, |(_, every)| *every)
        };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a count of buckets fits an i64"
        )]
        let gap = |at: f64| {
            GAP_BUCKETS.max((f64::from(PERIODS_STALE) * every_at(at) / width).ceil() as i64)
        };
        let (lower, upper) = (text_bound("?1"), text_bound("?2 + 1"));
        let sql = format!(
            // As the receivers' history buckets: divided by the span
            // plus one, so a reading exactly on `to` falls in the last
            // bucket.
            "SELECT s.name,
                    CAST((unixepoch(r.at) - ?1) * ?3 / (?4 + 1) AS INTEGER) AS bucket,
                    AVG(unixepoch(r.at, 'subsec')),
                    AVG(r.value)
             FROM reading r JOIN sensor s ON s.id = r.sensor_id
             WHERE s.quantity = ?5 AND r.at >= {lower} AND r.at < {upper}
             GROUP BY s.name, bucket
             ORDER BY s.name, bucket"
        );
        let mut statement = match self.conn.prepare(&sql) {
            Ok(statement) => statement,
            Err(e) if missing_table(&e) => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut rows = statement.query((from, to, points, span, quantity))?;
        let mut lines: Vec<Line> = Vec::new();
        // The previous bucket of the same line, and its time.
        let mut previous: Option<(i64, f64)> = None;
        while let Some(row) = rows.next()? {
            let name: String = row.get(0)?;
            let bucket: i64 = row.get(1)?;
            let at: f64 = row.get(2)?;
            let value: f64 = row.get(3)?;
            if lines.last().is_none_or(|line| line.name != name) {
                lines.push(Line {
                    name,
                    at: Vec::new(),
                    values: Vec::new(),
                });
                previous = None;
            }
            let line = lines.last_mut().expect("a line was just pushed");
            if previous.is_some_and(|(last, last_at)| bucket > last + gap(last_at)) {
                line.at.push(at - width);
                line.values.push(None);
            }
            previous = Some((bucket, at));
            line.at.push(at);
            line.values.push(Some(value));
        }
        Ok(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::SensorLog;
    use super::TABLES;
    use crate::schema::META;
    use crate::schema::stored;
    use crate::scratch::Scratch;

    const T0: i64 = 1_791_000_000;

    fn at(seconds: i64) -> String {
        stored(jiff::Timestamp::from_second(T0 + seconds).expect("a time"))
    }

    /// A log of two temperature sensors and one humidity, read every
    /// ten seconds, `room` silent for a minute in the middle.
    fn written(scratch: &Scratch) {
        let conn = scratch.connect();
        conn.execute_batch(META).expect("meta");
        conn.execute_batch(TABLES).expect("tables");
        conn.execute("INSERT INTO period VALUES (?1, 10)", [at(-3600)])
            .expect("every");
        conn.execute_batch(
            "INSERT INTO sensor VALUES (1, 'room', 'temperature', 'C');
             INSERT INTO sensor VALUES (2, 'bench', 'temperature', 'C');
             INSERT INTO sensor VALUES (3, 'room', 'humidity', '%RH');",
        )
        .expect("sensors");
        conn.execute(
            "INSERT INTO source VALUES (1, ?1, '/sys/a/temp1_input', 'sht4x')",
            [at(0)],
        )
        .expect("a source");
        conn.execute(
            "INSERT INTO source VALUES (1, ?1, '/sys/b/temp1_input', 'tmp117')",
            [at(500)],
        )
        .expect("a later source");
        for t in (0..600).step_by(10) {
            if !(200..260).contains(&t) {
                conn.execute("INSERT INTO reading VALUES (1, ?1, 21.0)", [at(t)])
                    .expect("room");
            }
            conn.execute("INSERT INTO reading VALUES (2, ?1, 23.0)", [at(t)])
                .expect("bench");
            conn.execute("INSERT INTO reading VALUES (3, ?1, 40.0)", [at(t)])
                .expect("humidity");
        }
    }

    #[test]
    fn sensors_are_listed_with_their_latest_source() {
        let scratch = Scratch::new("list");
        written(&scratch);
        let log = SensorLog::open(scratch.path()).expect("open");
        let sensors = log.sensors().expect("sensors");
        let named: Vec<_> = sensors
            .iter()
            .map(|s| (s.name.as_str(), s.quantity.as_str(), s.device.as_deref()))
            .collect();
        assert_eq!(
            named,
            [
                ("room", "humidity", None),
                ("bench", "temperature", None),
                ("room", "temperature", Some("tmp117")),
            ]
        );
        assert_eq!(log.every(), Some(std::time::Duration::from_secs(10)));
        let (first, last) = log.extent().expect("extent").expect("readings");
        assert_eq!((first, last), ((T0) as f64, (T0 + 590) as f64));
    }

    #[test]
    fn a_sensor_never_read_is_not_listed() {
        let scratch = Scratch::new("unread");
        written(&scratch);
        scratch
            .connect()
            .execute(
                "INSERT INTO sensor VALUES (4, 'nowhere', 'temperature', 'C')",
                [],
            )
            .expect("a sensor never read");
        let log = SensorLog::open(scratch.path()).expect("open");
        let names: Vec<_> = log
            .sensors()
            .expect("sensors")
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert!(!names.contains(&"nowhere".to_owned()), "{names:?}");
        assert_eq!(names.len(), 3);
    }

    #[test]
    fn a_quantity_is_one_line_per_sensor_broken_only_at_a_real_gap() {
        let scratch = Scratch::new("series");
        written(&scratch);
        let log = SensorLog::open(scratch.path()).expect("open");
        // Buckets of about two seconds: four or five empty between any
        // two readings, which must not break a line.
        let lines = log
            .series("temperature", T0, T0 + 600, 300)
            .expect("series");
        let names: Vec<_> = lines.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["bench", "room"]);
        let breaks = |name: &str| {
            lines
                .iter()
                .find(|l| l.name == name)
                .expect("a line")
                .values
                .iter()
                .filter(|v| v.is_none())
                .count()
        };
        assert_eq!(breaks("bench"), 0);
        assert_eq!(breaks("room"), 1);
        assert!(
            log.series("pressure", T0, T0 + 600, 300)
                .expect("none")
                .is_empty()
        );
    }

    #[test]
    fn a_log_with_no_tables_reads_as_empty() {
        let scratch = Scratch::new("empty");
        scratch.connect().execute_batch(META).expect("meta");
        let log = SensorLog::open(scratch.path()).expect("open");
        assert!(log.sensors().expect("sensors").is_empty());
        assert!(
            log.series("temperature", T0, T0 + 600, 300)
                .expect("series")
                .is_empty()
        );
        assert_eq!(log.every(), None);
        assert_eq!(log.extent().expect("extent"), None);
    }

    #[test]
    fn older_readings_are_judged_by_the_period_they_were_read_at() {
        let scratch = Scratch::new("periods");
        let conn = scratch.connect();
        conn.execute_batch(META).expect("meta");
        conn.execute_batch(TABLES).expect("tables");
        conn.execute(
            "INSERT INTO sensor VALUES (1, 'room', 'temperature', 'C')",
            [],
        )
        .expect("a sensor");
        // A minute apart for half an hour, then every ten seconds.
        conn.execute("INSERT INTO period VALUES (?1, 60)", [at(0)])
            .expect("a period");
        conn.execute("INSERT INTO period VALUES (?1, 10)", [at(1800)])
            .expect("a later period");
        for t in (0..1800).step_by(60).chain((1800..3600).step_by(10)) {
            conn.execute("INSERT INTO reading VALUES (1, ?1, 21.0)", [at(t)])
                .expect("a reading");
        }
        let log = SensorLog::open(scratch.path()).expect("open");
        assert_eq!(log.every(), Some(std::time::Duration::from_secs(10)));
        let lines = log
            .series("temperature", T0, T0 + 3600, 1500)
            .expect("series");
        let breaks = lines[0].values.iter().filter(|v| v.is_none()).count();
        assert_eq!(breaks, 0, "the minute-apart readings were broken apart");
    }
}
