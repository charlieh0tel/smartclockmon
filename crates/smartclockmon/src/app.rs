//! What the monitor is showing.

use std::collections::VecDeque;

use smartclock::snapshot::Freshness;
use smartclock::snapshot::Snapshot;
use smartclock::types::EfcPercent;

use crate::source::Attachment;

/// How many EFC readings to keep for the trend.
///
/// At the daemon's one-second fast tier this is about twenty minutes,
/// which is enough to see the oscillator breathe with temperature but
/// nowhere near enough to see it age.  Ageing is what the SQLite log is
/// for; this is the live view.
const TREND_LEN: usize = 240;

/// Monitor state.
#[derive(Debug)]
pub(crate) struct App {
    /// The most recent reading, if any has arrived.
    pub(crate) snapshot: Option<Snapshot>,
    /// Recent EFC readings, oldest first.
    pub(crate) efc_trend: VecDeque<EfcPercent>,
    /// How the monitor is attached.
    pub(crate) attachment: Attachment,
    /// Set when the operator has asked to leave.
    pub(crate) quitting: bool,
    /// Whether to draw with line-drawing characters.
    pub(crate) unicode: bool,
}

impl App {
    /// A monitor with nothing received yet.
    pub(crate) fn new(attachment: Attachment, unicode: bool) -> Self {
        Self {
            snapshot: None,
            efc_trend: VecDeque::with_capacity(TREND_LEN),
            attachment,
            quitting: false,
            unicode,
        }
    }

    /// Take a new reading.
    pub(crate) fn accept(&mut self, snapshot: Snapshot) {
        // Only record EFC from a reading that describes the receiver.
        // A stale or disconnected snapshot repeats the last value, and
        // flattening the trend with repeats would hide a real change.
        if snapshot.freshness == Freshness::Live
            && let Some(efc) = snapshot.efc
            && self.efc_trend.back() != Some(&efc)
        {
            if self.efc_trend.len() == TREND_LEN {
                self.efc_trend.pop_front();
            }
            self.efc_trend.push_back(efc);
        }
        self.snapshot = Some(snapshot);
    }

    /// The span of the trend, for labelling the axis.
    pub(crate) fn efc_range(&self) -> Option<(f64, f64)> {
        let mut values = self.efc_trend.iter().map(|e| e.percent());
        let first = values.next()?;
        Some(values.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v))))
    }
}
