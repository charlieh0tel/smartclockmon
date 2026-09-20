//! What the monitor is showing.

use std::collections::VecDeque;

use smartclock::snapshot::Freshness;
use smartclock::snapshot::Snapshot;
use smartclock::types::EfcPercent;

use crate::history::History;
use crate::history::Log;
use crate::history::Window;
use crate::source::Attachment;

/// How many EFC readings to keep for the trend.
///
/// At the daemon's one-second fast tier this is about twenty minutes,
/// which is enough to see the oscillator breathe with temperature but
/// nowhere near enough to see it age.  Ageing is what the SQLite log is
/// for; this is the live view.
const TREND_LEN: usize = 240;

/// Which screen is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum View {
    /// Current state at a glance.
    Dashboard,
    /// Graphs over a longer span, read from the daemon's log.
    History,
}

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
    /// Whether to draw with box drawing and block elements.  An ASCII
    /// fallback exists for terminals that cannot render them.
    pub(crate) unicode: bool,
    /// Recent 1 PPS intervals in nanoseconds, oldest first.
    pub(crate) ti_trend: VecDeque<f64>,
    /// Which screen is showing.
    pub(crate) view: View,
    /// How far back the history graphs look.
    pub(crate) window: Window,
    /// The daemon's log, when there is one to read.
    pub(crate) log: Option<Log>,
    /// The last series read from it.
    pub(crate) history: History,
    /// Why the history is unavailable, if it is.
    pub(crate) history_error: Option<String>,
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
            ti_trend: VecDeque::with_capacity(TREND_LEN),
            view: View::Dashboard,
            window: Window::Hour,
            log: None,
            history: History::default(),
            history_error: None,
        }
    }

    /// Open the daemon's log, if the daemon named one.
    ///
    /// Direct mode has no log by design: it records nothing, and
    /// quietly creating a database would contradict what the header
    /// says.  The history view explains that rather than showing an
    /// empty graph.
    pub(crate) fn open_log(&mut self) {
        match &self.attachment {
            Attachment::Daemon {
                database: Some(path),
                ..
            } => match Log::open(std::path::Path::new(path)) {
                Ok(log) => self.log = Some(log),
                Err(e) => self.history_error = Some(e.to_string()),
            },
            Attachment::Daemon { database: None, .. } => {
                self.history_error = Some("the daemon did not say where its log is".to_owned());
            }
            Attachment::Direct { .. } => {
                self.history_error =
                    Some("direct mode records nothing; run smartclockd for history".to_owned());
            }
        }
    }

    /// Re-read the graphs.  Called on a timer, not every frame.
    pub(crate) fn refresh_history(&mut self, columns: usize) {
        let Some(log) = &self.log else { return };
        match log.read(self.window, columns) {
            Ok(history) => {
                self.history = history;
                self.history_error = None;
            }
            Err(e) => self.history_error = Some(e.to_string()),
        }
    }

    /// Note that the source went away, keeping the last values on
    /// screen but no longer presenting them as current.
    pub(crate) fn lost(&mut self, why: String) {
        if let Some(snapshot) = self.snapshot.as_mut() {
            snapshot.freshness = Freshness::Disconnected;
            snapshot.last_error = Some(why);
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
        // The interval is the loop's error signal, so its shape matters
        // more than any single reading: hunting, sawtooth and residual
        // frequency error all show there and nowhere else.
        if snapshot.freshness == Freshness::Live
            && let Some(interval) = snapshot.time_interval
        {
            if self.ti_trend.len() == TREND_LEN {
                self.ti_trend.pop_front();
            }
            self.ti_trend.push_back(interval.as_nanos());
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
