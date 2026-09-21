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

    /// Internal temperature in degrees Celsius.  Undocumented command.
    pub temperature: Option<f64>,
    /// Oven current.  Undocumented command.
    pub oven_current: Option<f64>,
    /// EFC as the raw 20-bit DAC code.  Undocumented command.
    pub efc_dac: Option<u32>,

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
}

/// How one tier is faring.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierState {
    /// When it last succeeded.  `None` before its first poll.
    pub at: Option<Timestamp>,
    /// What went wrong the last time it ran, if it did.
    pub error: Option<String>,
}

/// How each tier is faring, kept per tier rather than per snapshot.
///
/// A snapshot is built up one tier at a time, so a single timestamp and
/// a single freshness flag describe it badly.  The fast tier succeeding
/// every second re-stamped the whole snapshot `Live` while the status
/// screen underneath it went minutes stale, and after a link drop the
/// first successful fast poll relabelled an hour-old sky as current.
/// Anything that shows or records a field has to be able to ask how old
/// that particular field is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Polled {
    /// The short scalar queries.
    pub fast: TierState,
    /// The status screen and the holdover detail.
    pub medium: TierState,
    /// Position, date and counters.
    pub slow: TierState,
}

impl Polled {
    /// How `tier` is faring.
    pub fn get(&self, tier: Tier) -> &TierState {
        match tier {
            Tier::Fast => &self.fast,
            Tier::Medium => &self.medium,
            Tier::Slow => &self.slow,
        }
    }

    fn get_mut(&mut self, tier: Tier) -> &mut TierState {
        match tier {
            Tier::Fast => &mut self.fast,
            Tier::Medium => &mut self.medium,
            Tier::Slow => &mut self.slow,
        }
    }

    /// Record that `tier` has just succeeded, clearing only its own
    /// error: a tier that works says nothing about one that does not.
    pub fn succeeded(&mut self, tier: Tier, at: Timestamp) {
        let state = self.get_mut(tier);
        state.at = Some(at);
        state.error = None;
    }

    /// Record that `tier` has just failed, leaving the time of its last
    /// success alone, since the values it wrote are still that old.
    pub fn failed(&mut self, tier: Tier, why: String) {
        self.get_mut(tier).error = Some(why);
    }

    /// How long ago `tier` last succeeded, in seconds.
    pub fn age(&self, tier: Tier, now: Timestamp) -> Option<f64> {
        let at = self.get(tier).at?;
        Some((now - at).total(jiff::Unit::Second).unwrap_or(0.0))
    }

    /// Whatever went wrong most recently, across every tier.
    pub fn any_error(&self) -> Option<&str> {
        Tier::ALL
            .into_iter()
            .find_map(|tier| self.get(tier).error.as_deref())
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
            temperature: None,
            oven_current: None,
            efc_dac: None,
            holdover_duration: None,
            holdover_predicted: None,
            holdover_present: None,
            screen: None,
            position: None,
            date: None,
            log_count: None,
        }
    }

    /// Recompute the whole-snapshot flag from the per-tier state.
    ///
    /// The flag predates `Polled` and used to be whatever the last tier
    /// to run set it to, which contradicted the very thing `Polled` was
    /// added for: with the medium tier failing and the fast tier fine,
    /// it alternated Live and Stale every second, and the `freshness`
    /// column in the log alternated with it.  A snapshot is as current
    /// as its least current part.
    ///
    /// `Disconnected` is not decided here.  It means the link itself is
    /// gone, which no tier's result can say on its own.
    pub fn settle_freshness(&mut self) {
        self.freshness = match self.polled.any_error() {
            Some(_) => Freshness::Stale,
            None => Freshness::Live,
        };
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

#[cfg(test)]
mod tests {
    use super::Freshness;
    use super::Snapshot;
    use super::Tier;

    #[test]
    fn one_tier_failing_makes_the_whole_snapshot_stale() {
        // The flag used to be whatever the last tier to run set, so a
        // fast tier succeeding every second relabelled a snapshot whose
        // status screen had been failing for minutes as Live, once a
        // second, for as long as it went on.
        let now = jiff::Timestamp::now();
        let mut snapshot = Snapshot::new(now);
        for tier in Tier::ALL {
            snapshot.polled.succeeded(tier, now);
        }
        snapshot.settle_freshness();
        assert_eq!(snapshot.freshness, Freshness::Live);

        snapshot
            .polled
            .failed(Tier::Medium, "no status screen".to_owned());
        snapshot.settle_freshness();
        assert_eq!(snapshot.freshness, Freshness::Stale);

        // The fast tier going round again must not paper over it.
        snapshot.polled.succeeded(Tier::Fast, now);
        snapshot.settle_freshness();
        assert_eq!(snapshot.freshness, Freshness::Stale);

        // Only the tier that was failing can clear it.
        snapshot.polled.succeeded(Tier::Medium, now);
        snapshot.settle_freshness();
        assert_eq!(snapshot.freshness, Freshness::Live);
    }
}
