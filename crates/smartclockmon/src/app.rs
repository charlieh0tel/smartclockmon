//! What the monitor is showing.

use std::collections::VecDeque;

use smartclock::snapshot::Freshness;
use smartclock::snapshot::Tier;
use smartclock::task::Cadence;
use smartclock::types::EfcPercent;
use smartclock::wire::Reading;

use crate::history::History;
use crate::history::Log;
use crate::history::Window;
use crate::source::Attachment;
use crate::source::Console;
use crate::source::Policy;

/// How many EFC readings to keep for the trend.
///
/// At the daemon's one-second fast tier this is about twenty minutes,
/// which is enough to see the oscillator breathe with temperature but
/// nowhere near enough to see it age.  Ageing is what the SQLite log is
/// for; this is the live view.
const TREND_LEN: usize = 240;

/// How many of the receiver's notes to hold for the journal view.
///
/// The whole of its diagnostic log is 222 entries, so this shows all of
/// one with room for the events and errors beside it.
const JOURNAL_LEN: usize = 300;

/// Which screen is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum View {
    /// Current state at a glance.
    Dashboard,
    /// Graphs over a longer span, read from the daemon's log.
    History,
    /// What the receiver has recorded about itself.
    Journal,
    /// The satellites overhead.
    ///
    /// Its own view because it is the only one that costs the receiver
    /// a status screen, and so is read only while it is open.
    Sky,
}

/// Monitor state.
#[derive(Debug)]
pub(crate) struct App {
    /// The most recent reading, if any has arrived.
    pub(crate) snapshot: Option<Reading>,
    /// Recent EFC readings, oldest first.
    pub(crate) efc_trend: VecDeque<EfcPercent>,
    /// How the monitor is attached.
    pub(crate) attachment: Attachment,
    /// Set when the operator has asked to leave.
    pub(crate) quitting: bool,
    /// Recent 1 PPS intervals in nanoseconds, oldest first.
    pub(crate) ti_trend: VecDeque<f64>,
    /// Which screen is showing.
    pub(crate) view: View,
    /// How far back the history graphs look.
    pub(crate) window: Window,
    /// The daemon's log, when there is one to read.
    pub(crate) log: Option<Log>,
    /// Every receiver the log holds, most recently seen first.
    pub(crate) receivers: Vec<crate::history::Receiver>,
    /// Which of them the graphs and the journal are showing.
    ///
    /// `None` only while no log is open or the log names no receiver;
    /// otherwise the most recently seen, which is the attached unit on
    /// a bench where they are swapped.
    pub(crate) receiver: Option<i64>,
    /// The last series read from it.
    pub(crate) history: History,
    /// Why the history is unavailable, if it is.
    pub(crate) history_error: Option<String>,
    /// What the receiver recorded about itself, newest first.
    pub(crate) journal: Vec<crate::history::Note>,
    /// Where console commands go.
    pub(crate) console: Console,
    /// What the daemon says this client may do.
    pub(crate) policy: Policy,
    /// How often the daemon polls each tier, as it reported them.  The
    /// defaults are only defaults, so a pane deciding whether a field
    /// has gone quiet has to ask rather than assume.
    pub(crate) cadence: Cadence,
    /// Whether the console is taking keystrokes.
    pub(crate) console_open: bool,
    /// What has been typed into it.
    pub(crate) console_input: String,
    /// The last answer, or the reason there was none.
    pub(crate) console_reply: Option<String>,
}

impl App {
    /// A monitor with nothing received yet.
    pub(crate) fn new(
        attachment: Attachment,
        console: Console,
        policy: Policy,
        cadence: Cadence,
    ) -> Self {
        Self {
            snapshot: None,
            efc_trend: VecDeque::with_capacity(TREND_LEN),
            attachment,
            quitting: false,
            ti_trend: VecDeque::with_capacity(TREND_LEN),
            view: View::Dashboard,
            window: Window::Hour,
            log: None,
            journal: Vec::new(),
            receivers: Vec::new(),
            receiver: None,
            history: History::default(),
            history_error: None,
            console,
            policy,
            cadence,
            console_open: false,
            console_input: String::new(),
            console_reply: None,
        }
    }

    /// Send what has been typed, and clear the line.
    ///
    /// The daemon decides what is permitted, so a refusal comes back as
    /// the reply rather than being second-guessed here; the monitor
    /// showing its own idea of the policy would only disagree with the
    /// daemon eventually.
    pub(crate) fn submit(&mut self) {
        let scpi = self.console_input.trim().to_owned();
        self.console_input.clear();
        if scpi.is_empty() {
            return;
        }
        self.console_reply = match self.console.send(&scpi) {
            Ok(()) => Some(format!("{scpi}  ...")),
            Err(e) => Some(format!("{scpi}  {e}")),
        };
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
                Ok(log) => {
                    self.receivers = log.receivers().unwrap_or_default();
                    self.receiver = self.receivers.first().map(|r| r.id);
                    self.log = Some(log);
                }
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
        let Some(receiver) = self.receiver else {
            return;
        };
        match log.read(receiver, self.window, columns) {
            Ok(history) => {
                self.history = history;
                self.history_error = None;
            }
            Err(e) => self.history_error = Some(e.to_string()),
        }
    }

    /// Re-read the receiver's own records.
    ///
    /// Shares `history_error` with the graphs: both read the same file
    /// through the same handle, so a failure of one is a failure of the
    /// other and two separate messages would say the same thing twice.
    pub(crate) fn refresh_journal(&mut self) {
        let Some(log) = &self.log else { return };
        let Some(receiver) = self.receiver else {
            return;
        };
        match log.journal(receiver, JOURNAL_LEN) {
            Ok(journal) => {
                self.journal = journal;
                self.history_error = None;
            }
            Err(e) => self.history_error = Some(e.to_string()),
        }
    }

    /// Show the next receiver the log holds.
    ///
    /// Does nothing when there is one, which is the usual case: a key
    /// that appears to do nothing is better than one that silently
    /// reorders a single-unit view.
    pub(crate) fn next_receiver(&mut self) {
        if self.receivers.len() < 2 {
            return;
        }
        let at = self
            .receivers
            .iter()
            .position(|r| Some(r.id) == self.receiver)
            .unwrap_or(0);
        self.receiver = Some(self.receivers[(at + 1) % self.receivers.len()].id);
    }

    /// How the chosen receiver is named on screen, when there is a
    /// choice to be aware of.
    pub(crate) fn receiver_label(&self) -> Option<String> {
        if self.receivers.len() < 2 {
            return None;
        }
        self.receivers
            .iter()
            .find(|r| Some(r.id) == self.receiver)
            .map(crate::history::Receiver::label)
    }

    /// Note that the source went away, keeping the last values on
    /// screen but no longer presenting them as current.
    pub(crate) fn lost(&mut self, why: String) {
        if let Some(snapshot) = self.snapshot.as_mut() {
            snapshot.freshness = Freshness::Disconnected;
            for tier in Tier::ALL {
                snapshot.polled.failed(tier, &why);
            }
        }
    }

    /// Take a new reading.
    pub(crate) fn accept(&mut self, snapshot: Reading) {
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
            && let Some(interval) = snapshot.time_interval_ns
        {
            if self.ti_trend.len() == TREND_LEN {
                self.ti_trend.pop_front();
            }
            self.ti_trend.push_back(interval);
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
