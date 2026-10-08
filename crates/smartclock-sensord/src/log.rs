//! Writing the sensor log.
//!
//! One writer, the service, and any number of readers opening the file
//! directly, as with the receivers' logs.  The tables are
//! `smartclock_log::sensors`', so the readers read what this writes.

use std::path::Path;
use std::time::Duration;

use jiff::Timestamp;
use rusqlite::Connection;
use rusqlite::OptionalExtension as _;
use rusqlite::params;
use smartclock_log::error::Result;
use smartclock_log::sensors::INTEGER_TIMES;
use smartclock_log::sensors::TABLES;
use smartclock_log::sensors::TIMES;
use smartclock_log::sensors::VERSION;
use smartclock_log::timestamp::Stored;
use smartclock_log::writer;

use crate::sensor::Name;
use crate::sensor::Quantity;

/// The service's write connection.
#[derive(Debug)]
pub struct Log {
    conn: Connection,
}

impl Log {
    /// Open or create the log, refusing one a newer service wrote, and
    /// record how often the sensors are read from now, if that is not
    /// what was last recorded.
    pub fn open(path: &Path, every: Duration) -> Result<Self> {
        let mut conn = writer::open(path)?;
        let found = writer::stamped_version(&conn, VERSION, "smartclock-sensord")?;
        if found.is_some_and(|found| found < INTEGER_TIMES) {
            writer::convert_to_integer_times(&mut conn, TABLES, &TIMES, VERSION)?;
        }
        conn.execute_batch(TABLES)?;
        let last: Option<f64> = conn
            .query_row(
                "SELECT every FROM period ORDER BY since DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if last != Some(every.as_secs_f64()) {
            conn.execute(
                "INSERT INTO period (since, every) VALUES (?1, ?2)",
                params![Stored(Timestamp::now()), every.as_secs_f64()],
            )?;
        }
        writer::stamp(&conn, VERSION)?;
        Ok(Self { conn })
    }

    /// The row for a sensor's `quantity`, made on first sight.
    pub fn sensor_id(&self, name: &Name, quantity: Quantity) -> Result<i64> {
        self.conn.execute(
            "INSERT OR IGNORE INTO sensor (name, quantity, unit) VALUES (?1, ?2, ?3)",
            params![name.as_str(), quantity.name(), quantity.unit()],
        )?;
        Ok(self.conn.query_row(
            "SELECT id FROM sensor WHERE name = ?1 AND quantity = ?2",
            params![name.as_str(), quantity.name()],
            |row| row.get(0),
        )?)
    }

    /// Record where a sensor is read from, if that is not what was last
    /// recorded.
    pub fn note_source(
        &self,
        id: i64,
        at: Timestamp,
        source: &str,
        device: Option<&str>,
    ) -> Result<()> {
        let last: Option<(String, Option<String>)> = self
            .conn
            .query_row(
                "SELECT source, device FROM source WHERE sensor_id = ?1
                 ORDER BY since DESC LIMIT 1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if last
            .as_ref()
            .is_some_and(|(s, d)| s == source && d.as_deref() == device)
        {
            return Ok(());
        }
        self.conn.execute(
            "INSERT INTO source (sensor_id, since, source, device) VALUES (?1, ?2, ?3, ?4)",
            params![id, Stored(at), source, device],
        )?;
        Ok(())
    }

    /// Record one reading.
    pub fn record(&self, id: i64, at: Timestamp, value: f64) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO reading (sensor_id, at, value) VALUES (?1, ?2, ?3)",
            params![id, Stored(at), value],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Log;
    use crate::sensor::Name;
    use crate::sensor::Quantity;
    use jiff::Timestamp;
    use smartclock_log::error::Error;
    use smartclock_log::sensors::SensorLog;
    use std::path::PathBuf;
    use std::time::Duration;

    /// A log file, removed with its WAL files when this goes.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let scratch = Self(std::env::temp_dir().join(format!(
                "smartclock-sensord-log-{name}-{}.sqlite",
                std::process::id()
            )));
            scratch.wipe();
            scratch
        }

        fn wipe(&self) {
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            self.wipe();
        }
    }

    #[test]
    fn what_is_written_is_what_the_readers_read() {
        let scratch = Scratch::new("round");
        let log = Log::open(&scratch.0, Duration::from_secs(10)).expect("open");
        let name = Name::new("room").expect("a name");
        let source = "/sys/x/temp1_input";
        let id = log.sensor_id(&name, Quantity::Temperature).expect("id");
        assert_eq!(
            log.sensor_id(&name, Quantity::Temperature)
                .expect("the same id"),
            id
        );
        let t0 = Timestamp::from_second(1_791_000_000).expect("a time");
        log.note_source(id, t0, source, Some("sht4x"))
            .expect("source");
        // The same again is not a new row; a new device is.
        log.note_source(id, t0, source, Some("sht4x"))
            .expect("same");
        let t1 = Timestamp::from_second(1_791_000_010).expect("a time");
        log.note_source(id, t1, source, Some("lm75"))
            .expect("changed");
        for (n, t) in [t0, t1].into_iter().enumerate() {
            log.record(id, t, 21.0 + n as f64).expect("a reading");
        }
        let read = SensorLog::open(&scratch.0).expect("read it");
        let sensors = read.sensors().expect("sensors");
        assert_eq!(sensors.len(), 1);
        assert_eq!(sensors[0].device.as_deref(), Some("lm75"));
        assert_eq!(read.every(), Some(Duration::from_secs(10)));
        let lines = read
            .series("temperature", 1_791_000_000, 1_791_000_010, 16)
            .expect("series");
        assert_eq!(lines[0].values, [Some(21.0), Some(22.0)]);
    }

    #[test]
    fn a_log_from_a_newer_service_is_refused() {
        let scratch = Scratch::new("newer");
        drop(Log::open(&scratch.0, Duration::from_secs(10)).expect("open"));
        rusqlite::Connection::open(&scratch.0)
            .expect("reopen")
            .execute("UPDATE meta SET value = '99' WHERE key = 'schema'", [])
            .expect("restamp");
        assert!(matches!(
            Log::open(&scratch.0, Duration::from_secs(10)),
            Err(Error::NewerSchema { found: 99, .. })
        ));
    }

    #[test]
    fn a_schema_1_log_is_converted_and_keeps_its_readings() {
        let scratch = Scratch::new("schema-1");
        {
            let conn = rusqlite::Connection::open(&scratch.0).expect("create");
            conn.execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO meta VALUES ('schema', '1');
                 CREATE TABLE sensor (id INTEGER PRIMARY KEY, name TEXT NOT NULL,
                     quantity TEXT NOT NULL, unit TEXT NOT NULL, UNIQUE (name, quantity));
                 CREATE TABLE source (sensor_id INTEGER NOT NULL REFERENCES sensor(id),
                     since TEXT NOT NULL, source TEXT NOT NULL, device TEXT);
                 CREATE INDEX source_sensor ON source(sensor_id, since);
                 CREATE TABLE period (since TEXT NOT NULL, every REAL NOT NULL);
                 CREATE TABLE reading (sensor_id INTEGER NOT NULL REFERENCES sensor(id),
                     at TEXT NOT NULL, value REAL NOT NULL,
                     PRIMARY KEY (sensor_id, at)) WITHOUT ROWID;
                 INSERT INTO sensor VALUES (1, 'room', 'temperature', 'C');
                 INSERT INTO source VALUES (1, '2026-09-21T00:00:00.000000000Z', '/sys/x', 'lm75');
                 INSERT INTO period VALUES ('2026-09-21T00:00:00.000000000Z', 10);
                 INSERT INTO reading VALUES
                     (1, '2026-09-21T00:00:00.000000000Z', 21.0),
                     (1, '2026-09-21T00:00:10.000000000Z', 22.0);",
            )
            .expect("a schema 1 log");
        }
        drop(Log::open(&scratch.0, Duration::from_secs(10)).expect("convert it"));
        let read = SensorLog::open(&scratch.0).expect("read it");
        let t0 = "2026-09-21T00:00:00Z"
            .parse::<Timestamp>()
            .expect("a time")
            .as_second();
        let lines = read.series("temperature", t0, t0 + 10, 16).expect("series");
        assert_eq!(lines[0].values, [Some(21.0), Some(22.0)]);
        assert_eq!(
            read.sensors().expect("sensors")[0].device.as_deref(),
            Some("lm75")
        );
    }
}
