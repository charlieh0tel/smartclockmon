//! The on-disk transcript format shared by capture and replay.
//!
//! One JSON object per line, append-only, so a capture stays readable
//! with `grep` and can be dropped straight in as a test fixture.

use serde::Deserialize;
use serde::Serialize;

/// Which way bytes were moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Host to receiver.
    Tx,
    /// Receiver to host.
    Rx,
}

/// One read or write.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    /// Seconds since the capture began.  Kept so replay can reproduce
    /// the receiver's pacing, which matters for the status screen.
    pub t: f64,
    /// Direction of travel.
    pub dir: Direction,
    /// The bytes, decoded as UTF-8.
    pub data: String,
    /// Set when the bytes were not valid UTF-8 and `data` is therefore
    /// lossy.  SCPI traffic is ASCII, so this flags a line problem.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lossy: bool,
}

impl Record {
    /// Build a record, decoding lossily and noting whether it had to.
    pub fn new(t: f64, dir: Direction, bytes: &[u8]) -> Self {
        let data = String::from_utf8_lossy(bytes);
        Self {
            t,
            dir,
            lossy: matches!(data, std::borrow::Cow::Owned(_)),
            data: data.into_owned(),
        }
    }
}
