//! The reading as it crosses the socket.
//!
//! Deliberately not [`Snapshot`] itself.  Serialising the internal type
//! made the wire format track every rename inside the library, so a
//! refactor nobody thought of as a protocol change would quietly break
//! every client.  Converting into this type means such a rename is a
//! compile error in [`Reading::from`] instead, and the two can move
//! independently.
//!
//! Field names carry their units where the internal type left them
//! implicit, since a client reading JSON has no type to consult.

use jiff::Timestamp;
use serde::Deserialize;
use serde::Serialize;

use crate::rollover::ReceiverDate;
use crate::screen::Screen;
use crate::snapshot::Freshness;
use crate::snapshot::Snapshot;
use crate::types::EfcPercent;
use crate::types::Ffom;
use crate::types::HardwareCondition;
use crate::types::HoldoverWaitReason;
use crate::types::Position;
use crate::types::SmartClockMode;
use crate::types::Tfom;

/// One reading of a receiver, as clients see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    /// When the reading was taken.
    pub at: Timestamp,
    /// Whether it still describes the receiver.
    pub freshness: Freshness,

    /// Disciplining mode.
    pub mode: Option<SmartClockMode>,
    /// Time figure of merit.
    pub tfom: Option<Tfom>,
    /// Frequency figure of merit.
    pub ffom: Option<Ffom>,
    /// 1 PPS against GPS, in nanoseconds.
    pub time_interval_ns: Option<f64>,
    /// Oscillator control voltage, percent of range.
    pub efc: Option<EfcPercent>,
    /// Oscillator control voltage, raw 20-bit value.
    pub efc_raw: Option<u32>,
    /// Internal temperature, degrees Celsius.
    pub temperature_c: Option<f64>,
    /// Oven current, as the receiver reports it.
    pub oven_current: Option<f64>,
    /// Hardware condition register.
    pub hardware: Option<HardwareCondition>,
    /// Why the receiver has not left holdover.
    pub holdover_waiting: Option<HoldoverWaitReason>,
    /// Whether it is in holdover now.
    pub holdover_active: Option<bool>,
    /// Time spent in holdover, in seconds.
    pub holdover_seconds: Option<f64>,
    /// Predicted 24 hour holdover error, in seconds.
    pub holdover_predicted_s: Option<f64>,
    /// Error accumulated so far in holdover, in seconds.
    pub holdover_present_s: Option<f64>,

    /// Antenna position.
    pub position: Option<Position>,
    /// The receiver's date, with any rollover recorded.
    pub date: Option<ReceiverDate>,
    /// Diagnostic log entry count.
    pub log_count: Option<i64>,
    /// The scraped status screen, the only source of per-satellite
    /// elevation, azimuth and signal strength.
    pub screen: Option<Screen>,
    /// What went wrong on the last failed poll.
    pub last_error: Option<String>,
}

impl From<&Snapshot> for Reading {
    fn from(s: &Snapshot) -> Self {
        Self {
            at: s.at,
            freshness: s.freshness,
            mode: s.mode,
            tfom: s.tfom,
            ffom: s.ffom,
            time_interval_ns: s.time_interval.map(|v| v.as_nanos()),
            efc: s.efc,
            efc_raw: s.efc_dac,
            temperature_c: s.temperature,
            oven_current: s.oven_current,
            hardware: s.hardware,
            holdover_waiting: s.holdover_waiting,
            holdover_active: s.holdover_duration.map(|h| h.active),
            holdover_seconds: s.holdover_duration.map(|h| h.elapsed.as_secs()),
            holdover_predicted_s: s.holdover_predicted.map(|v| v.as_secs()),
            holdover_present_s: s.holdover_present.map(|v| v.as_secs()),
            position: s.position,
            date: s.date,
            log_count: s.log_count,
            screen: s.screen.clone(),
            last_error: s.last_error.clone(),
        }
    }
}
