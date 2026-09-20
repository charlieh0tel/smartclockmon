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
    subscribers: Arc<Mutex<Vec<Sender<Snapshot>>>>,
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

    /// Receive every snapshot published from now on.
    ///
    /// A subscriber that stops reading is dropped at the next publish
    /// rather than blocking the task: the receiver must never stall
    /// because a client went away.
    pub fn subscribe(&self) -> Receiver<Snapshot> {
        let (tx, rx) = channel();
        self.subscribers.lock().expect("subscriber mutex").push(tx);
        rx
    }

    /// Store a snapshot and hand it to every live subscriber.
    pub fn publish(&self, snapshot: Snapshot) {
        *self.latest.lock().expect("snapshot mutex") = Some(snapshot.clone());
        self.subscribers
            .lock()
            .expect("subscriber mutex")
            .retain(|tx| tx.send(snapshot.clone()).is_ok());
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
            snapshot.polled.failed(tier, why.to_owned());
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
        let (tx, rx) = sync_channel(1);
        let request = Request::Command {
            scpi: scpi.into(),
            answer: tx,
        };
        self.requests
            .send(request)
            .map_err(|_| Error::TaskStopped("nothing is serving its request queue"))?;
        rx.recv()
            .map_err(|_| Error::TaskStopped("it dropped the reply"))?
    }
}

/// Why a task stopped.
#[derive(Debug)]
pub enum Stopped {
    /// Every handle was dropped; nothing will ask again.
    HandlesDropped,
    /// The link failed repeatedly and the device should be reopened.
    LinkFailed(Error),
}

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
                Ok(request) => self.serve(request),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Stopped::HandlesDropped,
            }
        }
    }

    /// The tier that comes due soonest.
    fn next_due(&self) -> (Tier, Instant) {
        Tier::ALL
            .into_iter()
            .map(|t| (t, self.due[t as usize]))
            .min_by_key(|(_, at)| *at)
            .expect("at least one tier")
    }

    /// Run one tier, returning a reason to stop if the link has died.
    fn run_tier(&mut self, tier: Tier) -> Option<Stopped> {
        // A command served since the last poll failed and left the
        // receiver talking; clear the line before reading anything as a
        // measurement.
        if self.resync {
            self.resync = false;
            if let Err(e) = self.device.session().sync() {
                return Some(Stopped::LinkFailed(e));
            }
        }
        let now = Timestamp::now();
        let mut snapshot = self.shared.latest().unwrap_or_else(|| Snapshot::new(now));
        let outcome = self.device.poll(tier, &mut snapshot, now);
        self.due[tier as usize] = Instant::now() + self.cadence.of(tier);

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
                snapshot.freshness = Freshness::Stale;
                snapshot.polled.failed(tier, e.to_string());
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

    /// Run one submitted request.
    fn serve(&mut self, request: Request) {
        match request {
            Request::Command { scpi, answer } => {
                let outcome = self.device.session().query(&scpi);
                // A failed command leaves the receiver's reply still
                // travelling, and the next scheduled poll would read it
                // as its own answer: a TFOM reported as an FFOM, oven
                // current reported as temperature, recorded to the log
                // as a measurement.  run_tier resyncs for exactly this
                // reason; serving a client command must too.
                let failed = outcome.is_err();
                // A caller that gave up before the answer arrived is
                // not an error worth acting on.
                let _ = answer.send(outcome);
                if failed {
                    self.resync = true;
                }
            }
            Request::Refresh => {
                let now = Instant::now();
                for tier in Tier::ALL {
                    self.due[tier as usize] = now;
                }
            }
        }
    }
}
