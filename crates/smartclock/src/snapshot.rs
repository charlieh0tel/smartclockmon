//! One reading of a receiver's state.
//!
//! Fields are polled at different rates, so a snapshot is built up
//! rather than taken all at once: each tier refreshes its own fields
//! and leaves the rest as they were.  Everything is optional, because a
//! value can be missing either because it has not been polled yet or
//! because the receiver declined to give it.

use jiff::Timestamp;
use serde::Deserialize;
use serde::Serialize;

use crate::rollover::ReceiverDate;
use crate::screen::Screen;
use crate::types::EfcPercent;
use crate::types::Ffom;
use crate::types::HardwareCondition;
use crate::types::HoldoverDuration;
use crate::types::HoldoverWaitReason;
use crate::types::Position;
use crate::types::Seconds;
use crate::types::SmartClockMode;
use crate::types::Tfom;

/// How often a group of fields is refreshed.
///
/// The split is a link budget, not a preference.  The status screen is
/// about 1.8 KB, close to a second of wire time at 19200, so it cannot
/// share a one-second tier with anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Tier {
    /// Short scalar queries: mode, figures of merit, interval, EFC and
    /// the hardware register.
    Fast,
    /// The status screen and the holdover detail.
    Medium,
    /// Position, date and counters, which barely move.
    Slow,
}

impl Tier {
    /// Every tier, fastest first.
    pub const ALL: [Tier; 3] = [Tier::Fast, Tier::Medium, Tier::Slow];
}

/// Whether a snapshot still describes the receiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Freshness {
    /// Polled successfully.
    Live,
    /// The last poll failed, but the link is still open.
    Stale,
    /// The link is down and reconnection is being attempted.
    Disconnected,
}

/// A receiver's state as last read.
///
/// `Freshness` matters as much as the values.  A monitor that keeps
/// showing the last good numbers after the link drops is worse than one
/// that shows nothing, because the numbers look current.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// When this snapshot was last touched.
    pub at: Timestamp,
    /// Whether it still describes the receiver.
    pub freshness: Freshness,
    /// When each tier last succeeded.
    pub polled: Polled,

    /// Disciplining mode.
    pub mode: Option<SmartClockMode>,
    /// Time figure of merit.
    pub tfom: Option<Tfom>,
    /// Frequency figure of merit.
    pub ffom: Option<Ffom>,
    /// Interval between the receiver's 1 PPS and GPS.
    pub time_interval: Option<Seconds>,
    /// Oscillator control voltage as a share of range.
    pub efc: Option<EfcPercent>,
    /// The hardware condition register.
    pub hardware: Option<HardwareCondition>,
    /// Why the receiver has not left holdover.
    pub holdover_waiting: Option<HoldoverWaitReason>,

    /// Time in holdover, and whether it is running now.
    pub holdover_duration: Option<HoldoverDuration>,
    /// Predicted 24 hour holdover error.
    pub holdover_predicted: Option<Seconds>,
    /// Error accumulated so far in holdover.
    pub holdover_present: Option<Seconds>,
    /// The scraped status screen, the only source of per-satellite
    /// elevation, azimuth and signal strength.
    pub screen: Option<Screen>,

    /// Averaged antenna position.
    pub position: Option<Position>,
    /// The receiver's date, with any GPS week rollover recorded rather
    /// than folded in.
    pub date: Option<ReceiverDate>,
    /// Diagnostic log entry count.
    pub log_count: Option<i64>,

    /// What went wrong on the last failed poll, if anything.
    pub last_error: Option<String>,
}

/// When each tier last succeeded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Polled {
    /// Last successful fast tier.
    pub fast: Option<Timestamp>,
    /// Last successful medium tier.
    pub medium: Option<Timestamp>,
    /// Last successful slow tier.
    pub slow: Option<Timestamp>,
}

impl Polled {
    /// When `tier` last succeeded.
    pub fn get(&self, tier: Tier) -> Option<Timestamp> {
        match tier {
            Tier::Fast => self.fast,
            Tier::Medium => self.medium,
            Tier::Slow => self.slow,
        }
    }

    /// Record that `tier` has just succeeded.
    pub fn set(&mut self, tier: Tier, at: Timestamp) {
        match tier {
            Tier::Fast => self.fast = Some(at),
            Tier::Medium => self.medium = Some(at),
            Tier::Slow => self.slow = Some(at),
        }
    }
}

impl Snapshot {
    /// An empty snapshot, before anything has been polled.
    pub fn new(at: Timestamp) -> Self {
        Self {
            at,
            freshness: Freshness::Disconnected,
            polled: Polled::default(),
            mode: None,
            tfom: None,
            ffom: None,
            time_interval: None,
            efc: None,
            hardware: None,
            holdover_waiting: None,
            holdover_duration: None,
            holdover_predicted: None,
            holdover_present: None,
            screen: None,
            position: None,
            date: None,
            log_count: None,
            last_error: None,
        }
    }

    /// Whether the oscillator or the receiver is reporting a fault.
    pub fn has_fault(&self) -> bool {
        self.hardware.is_some_and(|h| !h.is_healthy())
    }

    /// Whether the receiver's calendar is behind by whole GPS epochs.
    pub fn has_rollover(&self) -> bool {
        self.date.is_some_and(|d| d.rollover().is_some())
    }
}
