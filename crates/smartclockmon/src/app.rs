//! What the monitor is showing.

use std::collections::VecDeque;

use jiff::Timestamp;
use smartclock::screen::Screen;
use smartclock::snapshot::Freshness;
use smartclock::snapshot::Tier;
use smartclock::task::Cadence;
use smartclock::types::EfcPercent;
use smartclock::wire::Reading;
use smartclock_log::reader::Receiver;

use crate::history::History;
use crate::history::Log;
use crate::history::Window;
use crate::source::Answer;
use crate::source::Attachment;
use crate::source::Console;
use crate::source::Policy;

/// How many fast-tier readings each trend keeps.
///
/// At the daemon's one-second fast tier this is about twenty minutes,
/// which is enough to see the oscillator breathe with temperature but
/// nowhere near enough to see it age.  Ageing is what the SQLite log is
/// for; this is the live view.
const TREND_LEN: usize = 240;

/// Append to a trend, dropping its oldest once it holds [`TREND_LEN`].
fn push_bounded<T>(trend: &mut VecDeque<T>, value: T) {
    if trend.len() == TREND_LEN {
        trend.pop_front();
    }
    trend.push_back(value);
}

/// How many of the receiver's journal lines to hold for the journal view.
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
    /// The receiver's status screen and the satellites overhead.
    ///
    /// Its own view because it is the only one that costs the receiver
    /// a status screen, and so is read only while it is open.
    Status,
    /// Allan deviation of the 1 PPS against GPS.
    Stability,
}

/// A status screen, and when it was read.
#[derive(Debug)]
pub(crate) struct ScreenRead {
    pub(crate) screen: Screen,
    pub(crate) at: Timestamp,
}

/// Monitor state.
#[derive(Debug)]
pub(crate) struct App {
    /// The most recent reading, if any has arrived.
    pub(crate) snapshot: Option<Reading>,
    /// The last status screen read, and when.
    ///
    /// Held here because it arrives on one snapshot only: the daemon
    /// delivers the screen with the snapshot that read it and keeps it
    /// out of every one after, so reading it off the newest snapshot
    /// would lose it at the next poll.  No tier polls the screen, so
    /// this is as old as the last read -- the status view's, or the
    /// daemon's own for its log every `--sky` seconds -- and
    /// every pane that shows from it says how old.
    pub(crate) last_screen: Option<ScreenRead>,
    /// Recent EFC readings, oldest first.
    pub(crate) efc_trend: VecDeque<EfcPercent>,
    /// How the monitor is attached.
    pub(crate) attachment: Attachment,
    /// Set when the operator has asked to leave.
    pub(crate) quitting: bool,
    /// Recent 1 PPS intervals in nanoseconds, oldest first.
    pub(crate) ti_trend: VecDeque<f64>,
    /// When the fast tier read the last values the trends took.
    last_fast: Option<Timestamp>,
    /// Which screen is showing.
    pub(crate) view: View,
    /// How far back the history graphs look.
    pub(crate) window: Window,
    /// The daemon's log, when there is one to read.
    pub(crate) log: Option<Log>,
    /// Every receiver the log holds, most recently seen first.
    pub(crate) receivers: Vec<Receiver>,
    /// Which of them the graphs and the journal are showing.
    ///
    /// `None` only while no log is open or the log names no receiver;
    /// otherwise the most recently seen, which is the attached unit on
    /// a bench where they are swapped, until the operator picks one.
    pub(crate) receiver: Option<i64>,
    /// Whether the operator chose [`App::receiver`], which then stays
    /// chosen rather than following whichever unit was seen last.
    pub(crate) receiver_picked: bool,
    /// The last series read from it.
    pub(crate) history: History,
    /// The Allan deviation, recomputed while its view is open.
    pub(crate) deviation: smartclock::adev::Curve,
    /// Why the history is unavailable, if it is.
    pub(crate) history_error: Option<String>,
    /// What the receiver recorded about itself, newest first.
    pub(crate) journal: Vec<crate::history::Line>,
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
    pub(crate) console_reply: Option<Answer>,
}

impl App {
    /// The last status screen read, however old.
    pub(crate) fn screen(&self) -> Option<&Screen> {
        self.last_screen.as_ref().map(|read| &read.screen)
    }

    /// A monitor with nothing received yet.
    pub(crate) fn new(
        attachment: Attachment,
        console: Console,
        policy: Policy,
        cadence: Cadence,
    ) -> Self {
        Self {
            snapshot: None,
            last_screen: None,
            efc_trend: VecDeque::with_capacity(TREND_LEN),
            attachment,
            quitting: false,
            ti_trend: VecDeque::with_capacity(TREND_LEN),
            last_fast: None,
            view: View::Dashboard,
            window: Window::Hour,
            log: None,
            journal: Vec::new(),
            receivers: Vec::new(),
            receiver: None,
            receiver_picked: false,
            history: History::default(),
            deviation: smartclock::adev::Curve::default(),
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
        self.console_reply = Some(match self.console.send(&scpi) {
            Ok(()) => Ok(format!("{scpi}  ...")),
            Err(e) => Err(format!("{scpi}  {e}")),
        });
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
                    self.log = Some(log);
                    self.receiver_picked = false;
                    self.find_receivers();
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

    /// Take what a daemon reached again says about itself.
    ///
    /// The log is reopened only if it moved: reopening an unchanged one
    /// would reset the receiver chosen for the history panes.
    pub(crate) fn reattached(
        &mut self,
        database: Option<String>,
        policy: Policy,
        cadence: Cadence,
    ) {
        self.policy = policy;
        self.cadence = cadence;
        // A new attachment may be another unit, and a trend that runs
        // on from the last one splices two oscillators together.
        self.efc_trend.clear();
        self.ti_trend.clear();
        self.last_fast = None;
        if let Attachment::Daemon {
            database: current, ..
        } = &mut self.attachment
            && *current != database
        {
            *current = database;
            self.log = None;
            self.history_error = None;
            self.open_log();
        }
    }

    /// Re-read which receivers the log holds, and follow the one seen
    /// most recently unless the operator picked another.
    ///
    /// On every refresh, not only when the log is opened: a fresh log
    /// names no receiver until the daemon writes its first row, and a
    /// unit swapped onto the same log is seen only by looking again.
    /// Read once, a fresh log left every history view blank for the
    /// life of the process, saying nothing.
    fn find_receivers(&mut self) {
        let Some(log) = &self.log else { return };
        match log.receivers() {
            Ok(receivers) => self.receivers = receivers,
            Err(e) => {
                self.history_error = Some(e.to_string());
                return;
            }
        }
        let still_there = |id: i64| self.receivers.iter().any(|r| r.id == id);
        if !(self.receiver_picked && self.receiver.is_some_and(still_there)) {
            self.receiver_picked = false;
            self.receiver = self.receivers.first().map(|r| r.id);
        }
        if self.receiver.is_none() {
            self.history_error = Some("the log names no receiver yet".to_owned());
        }
    }

    /// The log and the receiver to read, after looking again for
    /// receivers.
    fn chosen(&mut self) -> Option<(&Log, i64)> {
        self.find_receivers();
        Some((self.log.as_ref()?, self.receiver?))
    }

    /// Re-read the graphs.  Called on a timer, not every frame.
    pub(crate) fn refresh_history(&mut self, columns: usize) {
        let window = self.window;
        let Some((log, receiver)) = self.chosen() else {
            return;
        };
        match log.read(receiver, window, columns) {
            Ok(history) => {
                self.history = history;
                self.history_error = None;
            }
            Err(e) => self.history_error = Some(e.to_string()),
        }
    }

    /// Recompute the deviation for the current window.
    ///
    /// Only called while its view is open: it reads every 1 PPS
    /// reading in the window at full rate, which is the one query here
    /// that is not cheap.
    pub(crate) fn refresh_deviation(&mut self) {
        let window = self.window;
        let Some((log, receiver)) = self.chosen() else {
            return;
        };
        match log.deviation(receiver, window) {
            Ok(deviation) => {
                self.deviation = deviation;
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
        let Some((log, receiver)) = self.chosen() else {
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
        self.receiver_picked = true;
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
            .map(Receiver::label)
    }

    /// Note that the source went away, keeping the last values on
    /// screen but no longer presenting them as current.
    pub(crate) fn lost(&mut self, why: String) {
        // A screen from before the link dropped may not be this
        // receiver's by the time it comes back.
        self.last_screen = None;
        if let Some(snapshot) = self.snapshot.as_mut() {
            snapshot.freshness = Freshness::Disconnected;
            for tier in Tier::ALL {
                snapshot.polled.failed(tier, &why);
            }
        }
    }

    /// Take a new reading.
    pub(crate) fn accept(&mut self, snapshot: Reading) {
        // One point per fast-tier reading, so a trend's length is
        // time.  Every tier publishes a snapshot and the slower ones
        // restate the fast tier's last values, and a disconnected one
        // repeats them too; counting those put several points a
        // fraction of a second apart.  Dropping a value equal to the
        // last instead collapsed a steady EFC to nothing.
        let fast = snapshot.polled.fast.at;
        if snapshot.freshness != Freshness::Disconnected && fast.is_some() && fast > self.last_fast
        {
            self.last_fast = fast;
            if let Some(efc) = snapshot.efc {
                push_bounded(&mut self.efc_trend, efc);
            }
            // The interval is the loop's error signal, so its shape
            // matters more than any single reading: hunting, sawtooth
            // and residual frequency error all show there and nowhere
            // else.
            if let Some(interval) = snapshot.time_interval_ns {
                push_bounded(&mut self.ti_trend, interval);
            }
        }
        // As in `lost`, for a link that went down without the daemon
        // going away.
        if snapshot.freshness == Freshness::Disconnected {
            self.last_screen = None;
        }
        if let Some(screen) = snapshot.screen.clone() {
            self.last_screen = Some(ScreenRead {
                screen,
                at: snapshot.at,
            });
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

#[cfg(test)]
mod tests {
    use super::App;
    use crate::source::Attachment;
    use crate::source::Console;
    use crate::source::Policy;
    use jiff::SignedDuration;
    use jiff::Timestamp;
    use smartclock::snapshot::Freshness;
    use smartclock::snapshot::Snapshot;
    use smartclock::snapshot::Tier;
    use smartclock::task::Cadence;
    use smartclock::types::EfcPercent;
    use smartclock::wire::Reading;
    use std::time::Duration;

    #[test]
    fn a_daemon_reached_again_is_taken_at_its_word() {
        let mut app = App::new(
            Attachment::Daemon {
                socket: "/run/smartclockd.sock".to_owned(),
                database: Some("/var/lib/smartclockd/first.db".to_owned()),
            },
            Console::default(),
            Policy::default(),
            Cadence::default(),
        );
        let slower = Cadence {
            medium: Duration::from_secs(30),
            ..Cadence::default()
        };
        app.reattached(
            Some("/nonexistent/second.db".to_owned()),
            Policy {
                control: true,
                ..Policy::default()
            },
            slower,
        );
        assert_eq!(
            app.attachment,
            Attachment::Daemon {
                socket: "/run/smartclockd.sock".to_owned(),
                database: Some("/nonexistent/second.db".to_owned()),
            }
        );
        assert!(app.policy.control);
        assert_eq!(app.cadence.medium, Duration::from_secs(30));
        // The new log is the one opened, and it is not there.
        assert!(app.history_error.is_some());
    }

    #[test]
    fn a_receiver_the_log_names_after_it_was_opened_is_found() {
        let path =
            std::env::temp_dir().join(format!("smartclockmon-fresh-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let conn = rusqlite::Connection::open(&path).expect("make a log");
        conn.execute_batch(smartclock_log::schema::META)
            .expect("meta");
        conn.execute_batch(smartclock_log::schema::TABLES)
            .expect("the tables");
        let mut app = App::new(
            Attachment::Daemon {
                socket: "/run/smartclockd.sock".to_owned(),
                database: Some(path.display().to_string()),
            },
            Console::default(),
            Policy::default(),
            Cadence::default(),
        );
        app.open_log();
        app.refresh_journal();
        let before = (app.receiver, app.history_error.clone());

        conn.execute_batch(
            "INSERT INTO receiver (id, serial, first_seen, last_seen) VALUES (7, 'AAA', '', '');",
        )
        .expect("the daemon's first row");
        app.refresh_journal();
        let _ = std::fs::remove_file(&path);

        assert_eq!(
            before,
            (None, Some("the log names no receiver yet".to_owned()))
        );
        assert_eq!(app.receiver, Some(7));
        assert_eq!(app.history_error, None);
    }

    #[test]
    fn a_trend_takes_one_point_per_fast_reading_and_keeps_a_steady_value() {
        let mut app = App::new(
            Attachment::Direct {
                device: "/dev/null".to_owned(),
            },
            Console::default(),
            Policy::default(),
            Cadence::default(),
        );
        let start = Timestamp::now();
        let reading = |fast_second: i64, published_ms: i64| {
            let fast = start + SignedDuration::from_secs(fast_second);
            let mut s = Snapshot::new(fast + SignedDuration::from_millis(published_ms));
            s.polled.succeeded(Tier::Fast, fast);
            s.efc = EfcPercent::new(1.0);
            s.freshness = Freshness::Live;
            Reading::from(&s)
        };
        // Two fast readings of the same EFC, each restated by a slower
        // tier's publication half a second later.
        for (fast, published) in [(0, 0), (0, 500), (1, 0), (1, 500)] {
            app.accept(reading(fast, published));
        }
        assert_eq!(app.efc_trend.len(), 2);
        // A reattach may be another unit, whose trend starts afresh.
        app.reattached(None, Policy::default(), Cadence::default());
        assert!(app.efc_trend.is_empty());
        app.accept(reading(2, 0));
        assert_eq!(app.efc_trend.len(), 1);
    }
}
