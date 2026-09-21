//! The thread that owns the receiver.
//!
//! One `DeviceTask` holds the [`Session`], and therefore the serial
//! file descriptor, for as long as it runs.  It is the only thing that
//! ever issues a command.  Everything else subscribes to the snapshots
//! it publishes or submits requests to its queue.
//!
//! The framing forces this.  A reply read only part way leaves the next
//! read starting mid-prompt, and every exchange after that is
//! misaligned -- observed on a 58503A, where an abandoned log dump made
//! the rest of a probe report each command with its predecessor's
//! answer.  Since [`Session`] is moved into this thread at
//! construction, a second writer is a compile error rather than a
//! corrupted session.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::mpsc::Sender;
use std::sync::mpsc::SyncSender;
use std::sync::mpsc::TryRecvError;
use std::sync::mpsc::TrySendError;
use std::sync::mpsc::channel;
use std::sync::mpsc::sync_channel;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use jiff::Timestamp;

use crate::device::Device;
use crate::error::Error;
use crate::error::Result;
use crate::session::Reply;
use crate::snapshot::Freshness;
use crate::snapshot::Snapshot;
use crate::snapshot::Tier;
use crate::transport::Transport;

/// How often each tier runs.
#[derive(Debug, Clone)]
pub struct Cadence {
    /// Short scalar queries.
    pub fast: Duration,
    /// The status screen and holdover detail.
    pub medium: Duration,
    /// Position, date and counters.
    pub slow: Duration,
}

impl Default for Cadence {
    fn default() -> Self {
        // The medium tier carries the status screen, roughly a second
        // of wire time at 19200, so it cannot run much faster than this
        // without starving everything else.
        Self {
            fast: Duration::from_secs(1),
            medium: Duration::from_secs(10),
            slow: Duration::from_secs(60),
        }
    }
}

impl Cadence {
    /// How often `tier` runs.
    pub fn of(&self, tier: Tier) -> Duration {
        match tier {
            Tier::Fast => self.fast,
            Tier::Medium => self.medium,
            Tier::Slow => self.slow,
        }
    }
}

/// Something asked of the task from outside.
#[derive(Debug)]
pub enum Request {
    /// Send a command and return its reply.
    Command {
        /// The SCPI string to send.
        scpi: String,
        /// When the caller stops waiting.  Past it the command is
        /// answered but not sent: a caller that has given up must not
        /// have its command executed behind its back.
        deadline: Instant,
        /// Where to put the answer.
        answer: SyncSender<Result<Reply>>,
    },
    /// Poll every tier at the next opportunity.
    ///
    /// Sent after a command that changed something, so the change shows
    /// in the snapshots at once rather than after up to a minute.
    Refresh,
}

/// State that outlives any one connection to the receiver.
///
/// Subscribers and the last known snapshot survive a reconnect, so a
/// client watching the daemon does not have to re-attach when the link
/// drops and comes back.
#[derive(Debug, Clone, Default)]
pub struct Shared {
    latest: Arc<Mutex<Option<Snapshot>>>,
    /// Watchers that may be dropped for falling behind.
    subscribers: Arc<Mutex<Vec<SyncSender<Snapshot>>>>,
    /// Watchers that may not: see [`Shared::subscribe_lossless`].
    recorders: Arc<Mutex<Vec<SyncSender<Snapshot>>>>,
}

impl Shared {
    /// Fresh state, before anything has been polled.
    pub fn new() -> Self {
        Self::default()
    }

    /// The most recent snapshot, if there has been one.
    pub fn latest(&self) -> Option<Snapshot> {
        self.latest.lock().expect("snapshot mutex").clone()
    }

    /// Receive the snapshots published from now on.
    ///
    /// The queue is bounded, and a subscriber that lets it fill is
    /// dropped rather than being allowed to grow it.  An unbounded
    /// channel only fails when the receiver has been *dropped*, so a
    /// client that merely stopped reading -- suspended, or written to
    /// only ever write -- queued a full snapshot per second in daemon
    /// memory for as long as it held the connection.  Falling behind
    /// costs a subscriber its subscription; it must not cost the daemon
    /// its memory.
    pub fn subscribe(&self) -> Receiver<Snapshot> {
        let (tx, rx) = sync_channel(SUBSCRIBER_BACKLOG);
        self.subscribers.lock().expect("subscriber mutex").push(tx);
        rx
    }

    /// Receive every snapshot, with a subscription that is never
    /// dropped for falling behind.
    ///
    /// For the one subscriber whose job is to write the history down.
    /// The asymmetry is the point: a watcher dropped for falling behind
    /// reconnects, whereas the log writer dropped for a stall in SQLite
    /// -- four contended writes at the five second busy timeout will do
    /// it -- stops recording for good, with the daemon still running
    /// and nothing saying so.
    ///
    /// The queue is still bounded, and generously: unbounded traded a
    /// silent stop for an unbounded heap, which on a daemon meant to
    /// run for months is the worse of the two.  A recorder this far
    /// behind has something wrong with it that losing a snapshot will
    /// not make worse, and [`Shared::publish`] says so out loud.
    pub fn subscribe_lossless(&self) -> Receiver<Snapshot> {
        let (tx, rx) = sync_channel(RECORDER_BACKLOG);
        self.recorders.lock().expect("recorder mutex").push(tx);
        rx
    }

    /// Store a snapshot and hand it to every live subscriber.
    ///
    /// `try_send` rather than `send`: a full queue means that
    /// subscriber has stopped reading, and blocking here would stop the
    /// receiver being polled at all.
    pub fn publish(&self, snapshot: Snapshot) {
        *self.latest.lock().expect("snapshot mutex") = Some(snapshot.clone());
        self.subscribers
            .lock()
            .expect("subscriber mutex")
            .retain(|tx| tx.try_send(snapshot.clone()).is_ok());
        // A recorder keeps its subscription whatever happens: it is
        // only removed when its receiver has gone, which means the
        // thread that was writing the log has exited.  A full queue
        // costs this one snapshot and a complaint, not the recording of
        // every snapshot after it.
        self.recorders.lock().expect("recorder mutex").retain(|tx| {
            match tx.try_send(snapshot.clone()) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) => {
                    eprintln!(
                        "smartclock: the recorder is {RECORDER_BACKLOG} snapshots behind; \
                         dropping this one"
                    );
                    true
                }
                Err(TrySendError::Disconnected(_)) => false,
            }
        });
    }

    /// Mark the last snapshot as no longer describing the receiver.
    ///
    /// Called when the link drops.  The values stay so a client can
    /// still show what was last true, but nothing may present them as
    /// current.
    pub fn mark_disconnected(&self, at: Timestamp, why: &str) {
        let mut snapshot = self.latest().unwrap_or_else(|| Snapshot::new(at));
        snapshot.at = at;
        snapshot.freshness = Freshness::Disconnected;
        for tier in Tier::ALL {
            snapshot.polled.failed(tier, why);
        }
        self.publish(snapshot);
    }
}

/// What a caller holds onto once the task is running.
#[derive(Debug, Clone)]
pub struct Handle {
    requests: Sender<Request>,
    shared: Shared,
}

impl Handle {
    /// Build a handle over an existing request channel and shared
    /// state, for a supervisor that outlives any one task.
    pub fn new(requests: Sender<Request>, shared: Shared) -> Self {
        Self { requests, shared }
    }

    /// The state shared with any client.
    pub fn shared(&self) -> &Shared {
        &self.shared
    }

    /// The most recent snapshot, without waiting for the next one.
    pub fn latest(&self) -> Option<Snapshot> {
        self.shared.latest()
    }

    /// Receive every snapshot published from now on.
    pub fn subscribe(&self) -> Receiver<Snapshot> {
        self.shared.subscribe()
    }

    /// Ask the task to re-poll everything at once.
    pub fn refresh(&self) {
        // A task that has gone is not worth reporting here; the next
        // command will say so.
        let _ = self.requests.send(Request::Refresh);
    }

    /// Send one command and wait for its reply.
    ///
    /// The task services requests between scheduled polls, never during
    /// one, so the wait can be as long as the slowest poll in flight --
    /// about a second when the status screen is being read.
    pub fn request(&self, scpi: impl Into<String>) -> Result<Reply> {
        self.request_within(scpi, REQUEST_TIMEOUT)
    }

    /// Send one command and wait no longer than `within` for its reply.
    ///
    /// Waiting forever was wrong while the link was down: nothing
    /// drains the queue then, so a caller blocked until the receiver
    /// came back, which could be hours, and its thread and its socket
    /// stayed up the whole time.
    pub fn request_within(&self, scpi: impl Into<String>, within: Duration) -> Result<Reply> {
        let (tx, rx) = sync_channel(1);
        let request = Request::Command {
            scpi: scpi.into(),
            deadline: Instant::now() + within,
            answer: tx,
        };
        self.requests
            .send(request)
            .map_err(|_| Error::TaskStopped("nothing is serving its request queue"))?;
        match rx.recv_timeout(within) {
            Ok(reply) => reply,
            Err(RecvTimeoutError::Timeout) => Err(Error::Timeout {
                waited: within,
                seen: "the receiver did not answer; the link may be down".to_owned(),
            }),
            Err(RecvTimeoutError::Disconnected) => Err(Error::TaskStopped("it dropped the reply")),
        }
    }
}

/// Answer and discard everything queued, for a link that has gone.
///
/// Without this, commands submitted during an outage were run against
/// the receiver whenever it came back -- a survey the operator started
/// and abandoned hours earlier, kicked off on reconnect.  A command is
/// for the receiver that was attached when it was sent.
///
/// Free rather than a method: it touches neither the task nor its
/// transport, and as an associated function the only way to call it was
/// to name a transport that has nothing to do with it.
pub fn discard_queued(requests: &Receiver<Request>, why: &'static str) -> usize {
    let mut discarded = 0;
    while let Ok(request) = requests.try_recv() {
        if let Request::Command { answer, .. } = request {
            let _ = answer.send(Err(Error::TaskStopped(why)));
        }
        discarded += 1;
    }
    discarded
}

/// Why a task stopped.
#[derive(Debug)]
pub enum Stopped {
    /// Every handle was dropped; nothing will ask again.
    HandlesDropped,
    /// The link failed repeatedly and the device should be reopened.
    LinkFailed(Error),
}

/// How long a caller waits for a command before giving up.
///
/// Generous next to the slowest exchange -- a status screen is about a
/// second at 19200 -- and short next to an outage.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// How many snapshots a recorder may fall behind before one is lost.
///
/// Far deeper than a watcher's backlog, because a recorder is never
/// dropped and losing a snapshot means losing it from the history.  At
/// the one-second tier this is about twenty minutes of stalled SQLite,
/// which is a broken disk rather than a slow one.
const RECORDER_BACKLOG: usize = 1024;

/// How many queued commands are served before the schedule gets a turn.
///
/// Enough that an interactive client is answered without waiting on a
/// poll, small enough that a client looping on requests cannot stop the
/// polling: the two interleave rather than one starving the other.
const REQUESTS_PER_POLL: usize = 4;

/// How many snapshots a subscriber may fall behind before it is
/// dropped.
///
/// Enough to ride out a client pausing for a few seconds at the fast
/// cadence, small enough that a stalled one cannot hold much.
const SUBSCRIBER_BACKLOG: usize = 16;

/// How many link failures in a row before the link is called dead.
///
/// One is ordinary; a run of them means the port is gone and reopening
/// is the only way back.  Only failures that reopening could fix count:
/// see [`Error::is_link_failure`].  Counting parse errors and receiver
/// refusals too meant one unexpected reply put the daemon in a
/// five-second reconnect loop forever, logging nothing, while the
/// hardware was healthy.
const FAILURES_BEFORE_RECONNECT: u32 = 3;

/// Runs the poll schedule and serves the request queue.
#[derive(Debug)]
pub struct DeviceTask<T: Transport> {
    device: Device<T>,
    cadence: Cadence,
    shared: Shared,
    requests: Receiver<Request>,
    /// When each tier is next due.
    due: [Instant; 3],
    /// When each tier last ran, for breaking ties between equal
    /// deadlines.  See [`DeviceTask::next_due`].
    last_run: [Instant; 3],
    /// Consecutive failed polls.
    failures: u32,
    /// Set when a served command left the session mid-reply.
    resync: bool,
}

/// Start a task on its own thread and return a handle to it.
pub fn spawn<T: Transport + Send + 'static>(
    device: Device<T>,
    cadence: Cadence,
) -> (Handle, thread::JoinHandle<Device<T>>) {
    let (tx, rx) = channel();
    let shared = Shared::new();
    let handle = Handle {
        requests: tx,
        shared: shared.clone(),
    };
    let mut task = DeviceTask::new(device, cadence, shared, rx);
    let joiner = thread::Builder::new()
        .name("smartclock-device".to_owned())
        .spawn(move || {
            task.run();
            task.device
        })
        .expect("spawning the device thread");
    (handle, joiner)
}

impl<T: Transport> DeviceTask<T> {
    /// Build a task over state that may outlive it.
    pub fn new(
        device: Device<T>,
        cadence: Cadence,
        shared: Shared,
        requests: Receiver<Request>,
    ) -> Self {
        let now = Instant::now();
        Self {
            device,
            cadence,
            shared,
            requests,
            due: [now, now, now],
            last_run: [now, now, now],
            failures: 0,
            resync: false,
        }
    }

    /// Take the request channel back, so the next task can serve it.
    ///
    /// A reconnect builds a new task around a freshly opened receiver
    /// but must keep the same queue, or every client holding a handle
    /// would be writing to a channel nobody reads.
    pub fn into_requests(self) -> Receiver<Request> {
        self.requests
    }

    /// Poll and serve requests until the handles go or the link dies.
    pub fn run(&mut self) -> Stopped {
        loop {
            // Serve what is waiting before running a due tier, but
            // only a bounded batch of it.  Polls used to take absolute
            // priority, so a tier as slow as its own period was always
            // overdue and client commands were never served at all:
            // every caller blocked forever and the queue grew without
            // bound.  Draining without a bound inverted that -- a
            // handful of clients each holding one request outstanding
            // published no snapshots at all, logged nothing, and left
            // the last one labelled Live.  A cap makes the two
            // interleave: at worst one poll per REQUESTS_PER_POLL
            // commands, and at worst that many commands per poll.
            //
            // Disconnection has to be noticed here too.  When every tier
            // is permanently overdue the blocking wait below is never
            // reached, so it was the only thing watching for the handles
            // going away, and the task ran on after the last one had
            // been dropped.
            for _ in 0..REQUESTS_PER_POLL {
                match self.requests.try_recv() {
                    Ok(request) => {
                        if let Some(stopped) = self.serve(request) {
                            return stopped;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Stopped::HandlesDropped,
                }
            }

            // Read the schedule after serving, not before.  Serving
            // can outlast the wait that was computed before it, and a
            // Refresh served in that batch moves every deadline, so a
            // reading taken earlier describes a schedule that no longer
            // exists: the task would sleep out a delay that had already
            // expired on a tier that was by then overdue.
            let now = Instant::now();
            let (tier, due) = self.next_due();

            if due <= now {
                if let Some(stopped) = self.run_tier(tier) {
                    return stopped;
                }
                continue;
            }

            // Wait for a request, but no longer than the next poll.  A
            // request arriving in that window is served immediately,
            // which is what preempts a scheduled poll.
            match self.requests.recv_timeout(due - now) {
                Ok(request) => {
                    if let Some(stopped) = self.serve(request) {
                        return stopped;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Stopped::HandlesDropped,
            }
        }
    }

    /// The tier that comes due soonest.
    fn next_due(&self) -> (Tier, Instant) {
        // Ties go to whichever tier ran longest ago, not to whichever
        // comes first in Tier::ALL.  A Refresh sets all three deadlines
        // to the same instant, so ties are common, and a fixed order
        // meant the slow tier could be passed over indefinitely: every
        // refresh recreated the tie, fast and medium took their turns,
        // and another refresh arrived before slow ever got one.  A
        // control command triggers a refresh, so a client issuing them
        // steadily was enough.
        Tier::ALL
            .into_iter()
            .map(|t| (t, self.due[t as usize]))
            .min_by_key(|(t, at)| (*at, self.last_run[*t as usize]))
            .expect("at least one tier")
    }

    /// Clear the line if the last exchange left the receiver talking.
    ///
    /// Anything that reads a reply has to do this first, a poll and a
    /// served command alike, or it reads the previous exchange's late
    /// answer as its own.
    fn ensure_synced(&mut self) -> Option<Stopped> {
        if !self.resync {
            return None;
        }
        self.resync = false;
        match self.device.session().sync() {
            Ok(_) => None,
            Err(e) => Some(Stopped::LinkFailed(e)),
        }
    }

    /// Run one tier, returning a reason to stop if the link has died.
    fn run_tier(&mut self, tier: Tier) -> Option<Stopped> {
        if let Some(stopped) = self.ensure_synced() {
            return Some(stopped);
        }
        let now = Timestamp::now();
        let mut snapshot = self.shared.latest().unwrap_or_else(|| Snapshot::new(now));
        let outcome = self.device.poll(tier, &mut snapshot, now);
        // Measured from when the tier was due, not from when its poll
        // finished, or the period becomes cadence plus wire time and
        // the sampling of a drifting oscillator is uneven.  Clamped
        // forward when a poll overruns so a slow tier cannot accumulate
        // a backlog of missed deadlines.
        self.last_run[tier as usize] = Instant::now();
        let cadence = self.cadence.of(tier);
        let slot = &mut self.due[tier as usize];
        *slot += cadence;
        let now_monotonic = Instant::now();
        if *slot < now_monotonic {
            *slot = now_monotonic + cadence;
        }

        match outcome {
            Ok(()) => self.failures = 0,
            Err(e) => {
                // A parse error or a refusal is recorded against the
                // tier and retried on its next turn; only a failing
                // link counts toward giving up on the port.
                if e.is_link_failure() {
                    self.failures += 1;
                } else {
                    self.failures = 0;
                }
                // Keep the values but say they are no longer current.  A
                // monitor that goes on showing the last good numbers as
                // though they were fresh is worse than one showing
                // nothing.  The failure is recorded against this tier
                // alone: another tier succeeding must not clear it.
                snapshot.polled.failed(tier, &e.to_string());
                snapshot.settle_freshness();
                if e.is_link_failure() && self.failures >= FAILURES_BEFORE_RECONNECT {
                    self.shared.publish(snapshot);
                    return Some(Stopped::LinkFailed(e));
                }
                // One bad poll can leave the session mid-reply, so
                // resynchronise before trying the next.  A failure here
                // means the receiver is still talking and nothing after
                // it can be trusted, so it ends the task rather than
                // being discarded.
                if let Err(e) = self.device.session().sync() {
                    self.shared.publish(snapshot);
                    return Some(Stopped::LinkFailed(e));
                }
            }
        }
        self.shared.publish(snapshot);
        None
    }

    /// Run one submitted request, returning a reason to stop if the
    /// link has died under it.
    fn serve(&mut self, request: Request) -> Option<Stopped> {
        match request {
            Request::Command {
                scpi,
                deadline,
                answer,
            } => {
                // Nobody is waiting for this any more.  Sending it
                // anyway meant a command the caller was told had timed
                // out was executed regardless -- a holdover it thought
                // had failed, initiated seconds later, recorded in the
                // audit trail as "failed".
                let expired = |answer: &SyncSender<Result<Reply>>| {
                    let _ = answer.send(Err(Error::Timeout {
                        waited: Duration::ZERO,
                        seen: "the caller stopped waiting before this was sent".to_owned(),
                    }));
                };
                if Instant::now() >= deadline {
                    expired(&answer);
                    return None;
                }
                // A command served since the last exchange failed left
                // the receiver talking; clear the line before reading
                // this one's answer.
                if let Some(stopped) = self.ensure_synced() {
                    let _ = answer.send(Err(Error::TaskStopped(
                        "the link failed while resynchronising",
                    )));
                    return Some(stopped);
                }
                // Again, because resynchronising talks to the receiver
                // and can take as long as a whole exchange.  The check
                // has to hold at the moment of transmission, not just
                // when the request was picked up.
                if Instant::now() >= deadline {
                    expired(&answer);
                    return None;
                }
                let outcome = self.device.session().query(&scpi);
                // A failed command leaves the receiver's reply still
                // travelling, and whatever reads next would take it as
                // its own answer: a TFOM reported as an FFOM, oven
                // current reported as temperature, recorded to the log
                // as a measurement.
                let failed = outcome.is_err();
                // A caller that gave up before the answer arrived is
                // not an error worth acting on.
                let _ = answer.send(outcome);
                if failed {
                    self.resync = true;
                }
                None
            }
            Request::Refresh => {
                let now = Instant::now();
                for tier in Tier::ALL {
                    self.due[tier as usize] = now;
                }
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Request;
    use super::discard_queued;
    use crate::error::Error;
    use std::sync::mpsc::channel;
    use std::sync::mpsc::sync_channel;
    use std::time::Duration;
    use std::time::Instant;

    /// A command as a client's handle would submit it.
    fn command(
        scpi: &str,
    ) -> (
        Request,
        std::sync::mpsc::Receiver<crate::error::Result<crate::session::Reply>>,
    ) {
        let (tx, rx) = sync_channel(1);
        (
            Request::Command {
                scpi: scpi.to_owned(),
                deadline: Instant::now() + Duration::from_secs(60),
                answer: tx,
            },
            rx,
        )
    }

    #[test]
    fn a_discarded_command_is_answered_rather_than_left_waiting() {
        // The queue outlives the task, so a command still in it when
        // the link dies would otherwise run against whatever receiver
        // came back -- and its caller would wait the full timeout to
        // hear nothing.
        let (tx, rx) = channel();
        let (first, first_answer) = command(":SYNChronization:HOLDover:INITiate");
        let (second, second_answer) = command(":GPS:POSition:SURVey:STATe ONCE");
        tx.send(first).expect("queue the first");
        tx.send(second).expect("queue the second");
        tx.send(Request::Refresh).expect("queue a refresh");

        let discarded = discard_queued(&rx, "the link went down");
        assert_eq!(discarded, 3, "everything queued should be taken");

        for answer in [first_answer, second_answer] {
            match answer.try_recv() {
                Ok(Err(Error::TaskStopped(why))) => assert_eq!(why, "the link went down"),
                other => panic!("expected a TaskStopped answer, got {other:?}"),
            }
        }

        // And the queue is empty, not merely drained of commands.
        assert_eq!(discard_queued(&rx, "again"), 0);
    }

    #[test]
    fn discarding_an_empty_queue_is_not_an_error() {
        let (tx, rx) = channel::<Request>();
        assert_eq!(discard_queued(&rx, "nothing to do"), 0);
        drop(tx);
        // A closed queue is still nothing to discard, not a panic.
        assert_eq!(discard_queued(&rx, "nothing to do"), 0);
    }
}
