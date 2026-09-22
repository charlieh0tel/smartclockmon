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
use jiff::civil::Date;
use serde::Deserialize;
use serde::Serialize;

use crate::rollover::ReceiverDate;
use crate::screen::Screen;
use crate::snapshot::Freshness;
use crate::snapshot::Polled;
use crate::snapshot::Snapshot;
use crate::types::AlarmCondition;
use crate::types::EfcPercent;
use crate::types::Ffom;
use crate::types::HardwareCondition;
use crate::types::HoldoverCondition;
use crate::types::HoldoverWaitReason;
use crate::types::OperationCondition;
use crate::types::Position;
use crate::types::PowerupCondition;
use crate::types::SmartClockMode;
use crate::types::Tfom;
use crate::types::TimeOfDay;

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
    /// The oscillator temperature coefficient the receiver has learned.
    /// Units unknown; the trend is the point.
    pub oven_tempco: Option<f64>,
    /// Hardware condition register.
    pub hardware: Option<HardwareCondition>,
    /// The faults that register names, decoded.
    ///
    /// Decoded here rather than left to each client: the register is
    /// the field that says the oscillator has railed, and a client
    /// showing it as "hardware 192" has told the reader nothing.  The
    /// bit table belongs with the receiver, not copied into every
    /// program that displays it.
    pub hardware_faults: Vec<String>,
    /// Why the receiver has not left holdover.
    pub holdover_waiting: Option<HoldoverWaitReason>,

    /// The alarm condition register, raw.
    ///
    /// Carried whole, unlike the other registers, because this is the
    /// one a client is most likely to want to reason about itself: it
    /// is what the front-panel lamp is showing.
    pub alarm: Option<AlarmCondition>,
    /// Which status groups have something latched, named.
    pub alarm_summary: Vec<String>,
    /// Whether the receiver has anything latched at all.
    pub alarming: Option<bool>,
    /// The receiver reset its clock to match the satellites.
    ///
    /// The questionable group holds only this and a bit no one here
    /// sets, so the group summary names it exactly.  A step in the time
    /// output invalidates every interval measurement across it.
    pub time_reset: Option<bool>,

    /// Locked to GPS, from the operation condition register.
    ///
    /// The condition registers are decoded here and their raw words are
    /// not sent: a bit position is only meaningful against the manual,
    /// and a client showing "operation 90" has said nothing.  The words
    /// themselves are kept in the log, where a later reader can go back
    /// to them if a bit turns out to have been misread.
    pub locked: Option<bool>,
    /// Holding a surveyed position rather than surveying.
    pub position_hold: Option<bool>,
    /// The GPS 1 PPS is fit to discipline against.
    pub reference_valid: Option<bool>,
    /// The receiver's diagnostic log is near the point where it stops
    /// recording.
    pub log_almost_full: Option<bool>,
    /// Coming out of holdover.
    pub holdover_recovering: Option<bool>,
    /// Holdover has run past its configured threshold.
    pub holdover_exceeding_threshold: Option<bool>,
    /// A satellite has been tracked since powerup.
    pub first_satellite_tracked: Option<bool>,
    /// The oscillator oven has warmed up since powerup.
    pub oven_warm: Option<bool>,
    /// The date and time were set at the first lock after powerup.
    pub date_time_valid: Option<bool>,
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
    /// The receiver's date exactly as it reported it, with any
    /// rollover recorded beside it.
    pub date: Option<ReceiverDate>,
    /// That date with the rollover applied.
    ///
    /// Computed here rather than left to each client, for the reason
    /// the fault list is: it is arithmetic over a constant the receiver
    /// does not know, and every client repeating it is every client
    /// getting a chance to repeat it differently.  Equal to `date.raw`
    /// on a receiver whose calendar is right.
    pub date_corrected: Option<Date>,
    /// UTC as the receiver reports it, as of the last fast poll.
    pub time: Option<TimeOfDay>,
    /// Diagnostic log entry count.
    pub log_count: Option<i64>,
    /// The scraped status screen, the only source of per-satellite
    /// elevation, azimuth and signal strength.
    pub screen: Option<Screen>,

    /// How old each group of fields is, and what went wrong with it.
    ///
    /// Carried over the wire because a client cannot otherwise tell a
    /// reading taken a second ago from one taken before the last
    /// outage: they arrive in the same message under the same
    /// timestamp.
    pub polled: Polled,
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
            oven_tempco: s.oven_tempco,
            hardware: s.hardware,
            hardware_faults: s
                .hardware
                .map(|h| h.faults().map(|f| f.describe().to_owned()).collect())
                .unwrap_or_default(),
            holdover_waiting: s.holdover_waiting,
            alarm: s.alarm,
            alarm_summary: s
                .alarm
                .map(|a| a.named_bits().into_iter().map(str::to_owned).collect())
                .unwrap_or_default(),
            alarming: s.alarm.map(|a| !a.is_clear()),
            time_reset: s.alarm.map(AlarmCondition::questionable),
            locked: s.operation.map(OperationCondition::locked),
            position_hold: s.operation.map(OperationCondition::position_hold),
            reference_valid: s.operation.map(OperationCondition::reference_valid),
            log_almost_full: s.operation.map(OperationCondition::log_almost_full),
            holdover_recovering: s.holdover_state.map(HoldoverCondition::recovering),
            holdover_exceeding_threshold: s
                .holdover_state
                .map(HoldoverCondition::exceeding_threshold),
            first_satellite_tracked: s.powerup.map(PowerupCondition::first_satellite_tracked),
            oven_warm: s.powerup.map(PowerupCondition::oven_warm),
            date_time_valid: s.powerup.map(PowerupCondition::date_time_valid),
            holdover_active: s.holdover_duration.map(|h| h.active),
            holdover_seconds: s.holdover_duration.map(|h| h.elapsed.as_secs()),
            holdover_predicted_s: s.holdover_predicted.map(|v| v.as_secs()),
            holdover_present_s: s.holdover_present.map(|v| v.as_secs()),
            position: s.position,
            date: s.date,
            date_corrected: s.date.map(|d| d.corrected()),
            time: s.time,
            log_count: s.log_count,
            screen: s.screen.clone(),
            polled: s.polled.clone(),
        }
    }
}
