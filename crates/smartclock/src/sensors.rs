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

/// What a client asks the sensor service.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Op {
    /// The service itself: answers [`Info`].
    Info,
    /// Every configured sensor and its latest reading: answers
    /// [`Latest`].
    Latest,
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
