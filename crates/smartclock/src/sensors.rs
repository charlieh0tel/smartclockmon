//! The sensor service's requests and replies, as they cross its socket.
//!
//! In the same envelope and framing as the receiver daemon's
//! ([`crate::protocol`]), with requests and a version of their own.  The
//! service sends nothing unasked.  Here rather than in the service so
//! that its clients -- the exporter, the web view, the command line
//! tool, the monitor -- share one definition with it.

use jiff::Timestamp;
use serde::Deserialize;
use serde::Serialize;

use crate::protocol::Protocol;

/// Bumped when the shapes below change.
pub const VERSION: u32 = 1;

/// How often the service reads its sensors unless told otherwise, in
/// seconds: the receivers' medium tier.
pub const DEFAULT_EVERY_S: f64 = 10.0;

/// How many read periods without a reading make a sensor's last
/// reading too old to show as current, and a gap in its history worth
/// breaking a line at.
pub const PERIODS_STALE: u32 = 3;

/// What a client asks the sensor service.  Named apart from the
/// receiver daemon's requests, so a client pointed at the wrong socket
/// is refused rather than answered in the other service's terms.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Op {
    /// The service itself: answers [`Info`].
    SensorInfo,
    /// Every configured sensor and its latest reading: answers
    /// [`Latest`].
    SensorLatest,
}

impl Protocol for Op {
    const VERSION: u32 = VERSION;
}

/// What the service says about itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Info {
    /// The service's build.
    pub version: String,
    /// How often every sensor is read, in seconds.
    pub every_s: f64,
    /// Where its log is, so a client can open it read-only.
    pub log: String,
}

/// Every configured sensor and its latest reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Latest {
    /// How often every sensor is read, in seconds: a reading older than
    /// a few of these is not current.
    pub every_s: f64,
    /// One per configured sensor, in the order configured.
    pub readings: Vec<Reading>,
}

/// One sensor, and what its last read gave.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    /// What it is called.
    pub name: String,
    /// What it measures: `temperature`, `humidity` or `pressure`.
    pub quantity: String,
    /// The unit of `value`.
    pub unit: String,
    /// Where it is read from, as configured.
    pub source: String,
    /// When it last read, if it ever has.
    pub at: Option<Timestamp>,
    /// What it last read.
    pub value: Option<f64>,
    /// Why its latest read failed, if it did; `at` and `value` are then
    /// the last read that did not.
    pub error: Option<String>,
}

impl Reading {
    /// The value, if it is current at `now`: the latest read succeeded,
    /// and no more than [`PERIODS_STALE`] read periods of `every_s`
    /// seconds ago.  An older value is the last one read, and shown as
    /// current it would draw a sensor that has stopped as steady.
    pub fn current(&self, every_s: f64, now: Timestamp) -> Option<f64> {
        let at = self.at?;
        let age = now.duration_since(at).as_secs_f64();
        let fresh = age <= f64::from(PERIODS_STALE) * every_s;
        self.value.filter(|_| self.error.is_none() && fresh)
    }
}

#[cfg(test)]
mod tests {
    use super::Reading;
    use jiff::Timestamp;

    #[test]
    fn a_value_is_current_only_while_reads_succeed_and_are_recent() {
        let t0 = Timestamp::from_second(1_791_000_000).expect("a time");
        let later = |s: i64| Timestamp::from_second(1_791_000_000 + s).expect("a time");
        let reading = Reading {
            name: "room".to_owned(),
            quantity: "temperature".to_owned(),
            unit: "C".to_owned(),
            source: "/x".to_owned(),
            at: Some(t0),
            value: Some(21.0),
            error: None,
        };
        assert_eq!(reading.current(10.0, later(30)), Some(21.0));
        assert_eq!(reading.current(10.0, later(31)), None);
        let failing = Reading {
            error: Some("gone".to_owned()),
            ..reading.clone()
        };
        assert_eq!(failing.current(10.0, later(5)), None);
        let never = Reading {
            at: None,
            value: None,
            ..reading
        };
        assert_eq!(never.current(10.0, later(5)), None);
    }
}
