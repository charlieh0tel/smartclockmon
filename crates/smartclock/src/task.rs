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

/// A command submitted from outside the task.
#[derive(Debug)]
pub struct Request {
    /// The SCPI string to send.
    pub scpi: String,
    /// Where to put the answer.
    pub answer: SyncSender<Result<Reply>>,
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
        snapshot.last_error = Some(why.to_owned());
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

    /// Send one command and wait for its reply.
    ///
    /// The task services requests between scheduled polls, never during
    /// one, so the wait can be as long as the slowest poll in flight --
    /// about a second when the status screen is being read.
    pub fn request(&self, scpi: impl Into<String>) -> Result<Reply> {
        let (tx, rx) = sync_channel(1);
        let request = Request {
            scpi: scpi.into(),
            answer: tx,
        };
        self.requests
            .send(request)
            .map_err(|_| crate::error::Error::Replay("the device task has stopped".to_owned()))?;
        rx.recv().map_err(|_| {
            crate::error::Error::Replay("the device task dropped a reply".to_owned())
        })?
    }
}

/// Why a task stopped.
#[derive(Debug)]
pub enum Stopped {
    /// Every handle was dropped; nothing will ask again.
    HandlesDropped,
    /// The link failed repeatedly and the device should be reopened.
    LinkFailed(crate::error::Error),
}

/// How many polls in a row may fail before the link is called dead.
///
/// One failure is ordinary: a timeout, or a value the receiver declined
/// for a reason the parser did not expect.  A run of them means the
/// port is gone, and reopening is the only way back.
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
        let now = Timestamp::now();
        let mut snapshot = self.shared.latest().unwrap_or_else(|| Snapshot::new(now));
        let outcome = self.device.poll(tier, &mut snapshot, now);
        self.due[tier as usize] = Instant::now() + self.cadence.of(tier);

        match outcome {
            Ok(()) => self.failures = 0,
            Err(e) => {
                self.failures += 1;
                // Keep the values but say they are no longer current.  A
                // monitor that goes on showing the last good numbers as
                // though they were fresh is worse than one showing
                // nothing.
                snapshot.freshness = Freshness::Stale;
                snapshot.last_error = Some(e.to_string());
                if self.failures >= FAILURES_BEFORE_RECONNECT {
                    self.shared.publish(snapshot);
                    return Some(Stopped::LinkFailed(e));
                }
                // One bad poll can leave the session mid-reply, so
                // resynchronise before trying the next.
                let _ = self.device.session().sync();
            }
        }
        self.shared.publish(snapshot);
        None
    }

    /// Run one submitted command.
    fn serve(&mut self, request: Request) {
        let outcome = self.device.session().query(&request.scpi);
        // A caller that gave up before the answer arrived is not an
        // error worth acting on.
        let _ = request.answer.send(outcome);
    }
}
