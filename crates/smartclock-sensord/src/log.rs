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
use smartclock_log::schema::stored;
use smartclock_log::sensors::TABLES;
use smartclock_log::sensors::VERSION;
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
        let conn = writer::open(path)?;
        // Nothing to migrate from yet: schema 1 is the first.
        writer::stamped_version(&conn, VERSION, "smartclock-sensord")?;
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
                params![stored(Timestamp::now()), every.as_secs_f64()],
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
            params![id, stored(at), source, device],
        )?;
        Ok(())
    }

    /// Record one reading.
    pub fn record(&self, id: i64, at: Timestamp, value: f64) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO reading (sensor_id, at, value) VALUES (?1, ?2, ?3)",
            params![id, stored(at), value],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Log;
    use crate::sysfs::Interface;
    use crate::sysfs::parse;
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
        let sensor = parse(Interface::Hwmon, "room=/sys/x/temp1_input").expect("parse");
        let id = log.sensor_id(&sensor.name, sensor.quantity).expect("id");
        assert_eq!(
            log.sensor_id(&sensor.name, sensor.quantity)
                .expect("the same id"),
            id
        );
        let t0 = Timestamp::from_second(1_791_000_000).expect("a time");
        log.note_source(id, t0, &sensor.source, Some("sht4x"))
            .expect("source");
        // The same again is not a new row; a new device is.
        log.note_source(id, t0, &sensor.source, Some("sht4x"))
            .expect("same");
        let t1 = Timestamp::from_second(1_791_000_010).expect("a time");
        log.note_source(id, t1, &sensor.source, Some("lm75"))
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
}
