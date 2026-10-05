//! Logging daemon for one SmartClock receiver.
//!
//! Holds the serial port open for as long as it runs, so EFC and
//! holdover history keeps accumulating whether or not anyone is
//! watching.  Clients reach it over a local socket for live state, and
//! by opening the SQLite log read-only for history.

mod inbox;

mod db;
mod journal;
#[cfg(test)]
mod scratch;
mod server;

use crate::inbox::Entry;
use crate::inbox::Fact;
use crate::inbox::LogInbox;
use crate::inbox::LogRequest;
use crate::inbox::Note;
use crate::journal::Journal;
use crate::journal::LOST_TO_OVERFLOW;
use crate::server::Policy;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::mpsc::Sender;
use std::sync::mpsc::channel;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context as _;
use anyhow::Result;
use clap::Parser;
use interprocess::local_socket::GenericFilePath;
use interprocess::local_socket::Stream;
use interprocess::local_socket::ToFsName as _;
use interprocess::local_socket::traits::Stream as _;
use jiff::Timestamp;
use signal_hook::consts::SIGINT;
use signal_hook::consts::SIGTERM;
use signal_hook::iterator::Signals;
use smartclock::attach::attach;
use smartclock::command::Dialect;
use smartclock::device::Device;
use smartclock::session::Config;
use smartclock::snapshot::Freshness;
use smartclock::snapshot::Snapshot;
use smartclock::task;
use smartclock::task::Cadence;
use smartclock::task::DeviceTask;
use smartclock::task::Handle;
use smartclock::task::Request;
use smartclock::task::Shared;
use smartclock::task::Stopped;
use smartclock::transport::Transport;
use smartclock::transport::serial::Settings;
use smartclock::types::BaudRate;
use smartclock::types::Framing;

#[derive(Parser)]
#[command(about, version = smartclock::VERSION)]
struct Cli {
    /// Serial device, or `tcp://host:port` for a receiver on the
    /// network or the simulator.  Prefer a /dev/serial/by-id/... path
    /// for a local one; /dev/ttyUSB0 does not survive re-enumeration.
    ///
    /// Required, and deliberately without a default.  A default does
    /// not fail when it is wrong: it opens whatever else is on that
    /// path and starts sending SCPI at it.
    #[arg(long, env = "SMARTCLOCKD_DEVICE")]
    device: String,

    /// Bits per second.  Checked against the rates a port can be opened at.
    #[arg(long, env = "SMARTCLOCKD_BAUD", default_value_t = 19200)]
    baud: u32,

    /// Character framing, `8N1` or `7O1`.  The 58503A's is settable
    /// and 8N1 here; the Z3801A's is fixed at 7O1.  With --baud, the
    /// settings tried first: a receiver that does not answer at them is
    /// looked for at 19200 and 9600, 8N1 and 7O1.
    #[arg(long, env = "SMARTCLOCKD_FRAMING", default_value = "8N1")]
    framing: Framing,

    /// Where the logs live: one file per receiver, named
    /// `<model>-<serial>.sqlite` once the receiver has answered `*IDN?`,
    /// so a unit keeps one history whichever port it is on.
    #[arg(
        long,
        env = "SMARTCLOCKD_LOG_DIR",
        default_value = "/var/lib/smartclockd"
    )]
    log_dir: PathBuf,

    /// One fixed log file for every receiver seen on this port, in
    /// place of a file per receiver in --log-dir.
    #[arg(long, env = "SMARTCLOCKD_DATABASE")]
    database: Option<PathBuf>,

    /// Where to listen for clients.  No default: the socket is per
    /// instance, and the unit sets it from the instance name.
    #[arg(long, env = "SMARTCLOCKD_SOCKET")]
    socket: PathBuf,

    /// Seconds between fast-tier polls.
    #[arg(long, env = "SMARTCLOCKD_FAST", default_value_t = 1.0)]
    fast: f64,

    /// Seconds between satellite, oscillator and holdover polls.
    ///
    /// Ten rather than thirty because this tier no longer carries the
    /// status screen, which was all that made it expensive.  Its four
    /// steps measure 0.57 s together and run one per turn, against a
    /// fast pass of 0.38 s in a one-second slot -- so the oven
    /// temperature and the satellite counts can be read six times as
    /// often for wire time the link has to spare.
    #[arg(long, env = "SMARTCLOCKD_MEDIUM", default_value_t = 10.0)]
    medium: f64,

    /// Seconds between position and date polls.
    #[arg(long, env = "SMARTCLOCKD_SLOW", default_value_t = 60.0)]
    slow: f64,

    /// Seconds between status-screen reads, so the log's satellite
    /// table -- elevation, azimuth and signal, which only the screen
    /// carries -- is kept without a client looking at the sky view.
    /// A read costs about 1.5 s of link, so the default of 300 is half
    /// a percent of it.  0 reads the screen only when a client asks.
    #[arg(long, env = "SMARTCLOCKD_SKY", default_value_t = 300.0)]
    sky: f64,

    /// Permit commands that change receiver state: holdover, survey,
    /// antenna delay, elevation mask.
    #[arg(long, env = "SMARTCLOCKD_ALLOW_CONTROL")]
    allow_control: bool,

    /// Permit commands that can strand the link or wipe configuration:
    /// system preset, serial reconfiguration, flash erase, language
    /// change.  A baud change persists across power cycles.
    #[arg(long, env = "SMARTCLOCKD_ALLOW_DANGEROUS")]
    allow_dangerous: bool,

    /// Erase the receiver's own diagnostic log once every entry has
    /// been copied into this one.
    ///
    /// Off by default, and deliberately.  The receiver's log holds 222
    /// entries and then stops recording, so a unit left alone has
    /// usually stopped logging long ago -- clearing it is the only way
    /// to get it recording again.  But erasing is irreversible and
    /// non-volatile, and on a unit somebody is investigating that log
    /// is evidence.  Nothing is erased until the copy here is complete
    /// and gap-free, and the count is passed with the command so the
    /// receiver refuses if an entry arrived in between.
    #[arg(long, env = "SMARTCLOCKD_ADOPT_LOG")]
    adopt_log: bool,

    /// Permit raw SCPI the command table does not recognise.  An
    /// unrecognised command is treated as control, or as dangerous if
    /// it resembles one that can strand the link.
    #[arg(long, env = "SMARTCLOCKD_ALLOW_RAW")]
    allow_raw: bool,
}

/// How often the log thread looks for a command to record when no
/// snapshot has arrived to wake it.
const AUDIT_POLL: Duration = Duration::from_millis(200);

/// The exit status for a mistake no retry can fix.
///
/// The same code clap uses for a usage error, and the unit's
/// `RestartPreventExitStatus=`, so anything that says "what you asked
/// for is wrong" stops rather than looping.
const CONFIGURATION_ERROR: i32 = 2;

/// How long to wait before reopening a receiver that went away.
const RECONNECT_DELAY: Duration = Duration::from_secs(5);

/// How often a wait checks whether the daemon has been told to stop.
const STOP_POLL: Duration = Duration::from_millis(100);

/// How long after startup the receiver's own records are first read.
const FIRST_JOURNAL: Duration = Duration::from_secs(5);

/// How often they are read after that.
///
/// The medium tier's period: an error the receiver raises is read off
/// its queue, and logged, within ten seconds of when it happened, and a
/// diagnostic log being copied takes a few entries a pass.  The event
/// registers are not read: reading one clears it, which a logger has no
/// business doing (`PLAN.md`, Phases, 10).  The engine's identity and
/// the transition filters are read once per attachment.
///
/// The cost when nothing has happened is two short queries, the error
/// queue and the diagnostic log count, which answer in one reply each
/// when empty and unchanged, nearly always.
const JOURNAL_EVERY: Duration = Duration::from_secs(10);

fn main() -> Result<()> {
    let cli = Cli::parse();
    let baud = BaudRate::new(cli.baud).with_context(|| {
        let supported = BaudRate::ALL.map(|b| b.to_string()).join(", ");
        format!(
            "{} is not a rate the receiver supports ({supported})",
            cli.baud
        )
    })?;

    // First line in the journal, so "which build is running" is
    // answerable from the logs alone rather than by finding the binary.
    eprintln!("smartclockd: version {}", smartclock::VERSION);

    // from_secs_f64 panics on a negative or non-finite value, so a
    // typo in a flag would abort rather than being reported.
    let cadence = Cadence {
        fast: seconds(cli.fast, "--fast")?,
        medium: seconds(cli.medium, "--medium")?,
        slow: seconds(cli.slow, "--slow")?,
    };
    // A database this binary cannot read is a configuration mistake,
    // not a transient failure, so it exits like one: retrying cannot
    // turn a newer schema into an older one, and Restart= would
    // otherwise reopen it every five seconds forever.  With a file per
    // receiver the same applies to each file as a receiver answers,
    // except that the daemon then keeps running without a log and
    // says so, since the other files may be fine.
    let (place, database) = match &cli.database {
        Some(path) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            let log = match db::Log::open(path) {
                Ok(log) => log,
                Err(e) => {
                    eprintln!("smartclockd: {e:#}");
                    std::process::exit(CONFIGURATION_ERROR);
                }
            };
            describe(&log, path)?;
            (Place::Fixed(Some(log)), path.display().to_string())
        }
        None => {
            std::fs::create_dir_all(&cli.log_dir)
                .with_context(|| format!("creating {}", cli.log_dir.display()))?;
            let mut existing: Vec<String> = std::fs::read_dir(&cli.log_dir)?
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".sqlite"))
                .collect();
            existing.sort();
            eprintln!(
                "smartclockd: logs in {}, one per receiver{}",
                cli.log_dir.display(),
                if existing.is_empty() {
                    String::new()
                } else {
                    format!(": {}", existing.join(", "))
                }
            );
            let place = Place::PerReceiver {
                dir: cli.log_dir.clone(),
                device: cli.device.clone(),
            };
            (place, String::new())
        }
    };

    let (inbox_tx, inbox) = channel::<LogRequest>();

    let shared = Shared::new();
    let (requests_tx, requests_rx) = channel();
    let stopping = watch_for_stop(Handle::new(requests_tx.clone(), shared.clone()))?;

    let policy = Policy {
        control: cli.allow_control,
        dangerous: cli.allow_dangerous,
        raw: cli.allow_raw,
    };
    if policy.control || policy.dangerous || policy.raw {
        eprintln!(
            "smartclockd: clients may change the receiver (control {}, dangerous {}, raw {})",
            policy.control, policy.dangerous, policy.raw
        );
    }
    // Built here rather than where the receiver is opened, because the
    // log thread needs the dialect too: the queries it sends are
    // resolved through the same table as every other one, and which
    // table that is only becomes known once a receiver has answered.
    let info: server::SharedInfo = Arc::new(Mutex::new(server::Info {
        identity: String::new(),
        dialect: Dialect::Hp58503,
        database: database.clone(),
        policy,
        inbox: LogInbox::new(inbox_tx),
        cadence: cadence.clone(),
    }));

    // Writing the log is a subscriber, so a slow or failing write
    // cannot hold up the receiver -- but not an ordinary one.  An
    // ordinary subscriber that falls behind is dropped, and for the
    // thread that writes the history that means logging stops for good
    // while the daemon runs on saying nothing.
    let writes = shared.subscribe_lossless();
    // The journal reaches the receiver the way any client does, through
    // the request queue, so it takes its turn between polls rather than
    // interrupting one.
    let journal_handle = Handle::new(requests_tx.clone(), shared.clone());
    let journal_info = Arc::clone(&info);
    let adopt_log = cli.adopt_log;
    if adopt_log {
        eprintln!(
            "smartclockd: the receiver's diagnostic log will be erased once it is \
             nearly full and every entry is held here"
        );
    }
    let log_cadence = cadence.clone();
    // Its own flag, set only once the task has stopped, so nothing the
    // task publishes or a command it ran can arrive after the thread
    // has drained and gone.
    let finished = Arc::new(AtomicBool::new(false));
    let log_finished = Arc::clone(&finished);
    let log_thread = thread::Builder::new()
        .name("smartclockd-log".to_owned())
        .spawn(move || {
            record(LogThread {
                place,
                cadence: log_cadence,
                adopt_log,
                writes,
                inbox,
                journal_handle,
                journal_info,
                finished: log_finished,
            });
        })
        .context("spawning the log thread")?;

    if cli.sky != 0.0 {
        let every = seconds(cli.sky, "--sky")?;
        read_sky(
            every,
            Handle::new(requests_tx.clone(), shared.clone()),
            Arc::clone(&stopping),
        )?;
    }

    let supervised = supervise(Supervisor {
        cli,
        baud,
        cadence,
        shared,
        requests_tx,
        requests_rx,
        info,
        stopping: Arc::clone(&stopping),
    });
    // The supervisor has gone, and with it the request queue, so a
    // journal pass in flight fails its next query at once rather than
    // waiting out a timeout.
    finished.store(true, Ordering::SeqCst);
    if log_thread.join().is_err() {
        eprintln!("smartclockd: the log thread panicked");
    }
    eprintln!("smartclockd: stopped");
    supervised
}

/// Read the status screen every `every`, as the sky view does.  The
/// screen reaches the log through the snapshot it is delivered on.  A
/// failure is reported once, not on every read while the link is down.
/// Stops when the daemon does, rather than reporting the request queue
/// gone as a failed read on the way out.
fn read_sky(every: Duration, handle: Handle, stopping: Arc<AtomicBool>) -> Result<()> {
    thread::Builder::new()
        .name("smartclockd-sky".to_owned())
        .spawn(move || {
            let mut failing = false;
            loop {
                pause(every, &stopping);
                if stopping.load(Ordering::SeqCst) {
                    return;
                }
                match handle.status() {
                    Ok(_) => failing = false,
                    Err(e) if !failing => {
                        eprintln!("smartclockd: reading the status screen for the sky: {e}");
                        failing = true;
                    }
                    Err(_) => {}
                }
            }
        })
        .context("spawning the sky thread")?;
    Ok(())
}

/// Stop on SIGTERM or SIGINT, and at once on a second one.
///
/// The first sets the returned flag and wakes the task so the
/// supervisor returns; the log thread then writes what it holds.  A
/// second signal is someone who will not wait for that.
fn watch_for_stop(handle: Handle) -> Result<Arc<AtomicBool>> {
    let stopping = Arc::new(AtomicBool::new(false));
    let mut signals = Signals::new([SIGTERM, SIGINT]).context("watching for signals")?;
    let flag = Arc::clone(&stopping);
    thread::Builder::new()
        .name("smartclockd-signals".to_owned())
        .spawn(move || {
            for signal in signals.forever() {
                if flag.swap(true, Ordering::SeqCst) {
                    eprintln!("smartclockd: signal {signal} again, exiting now");
                    std::process::exit(1);
                }
                eprintln!("smartclockd: signal {signal}, stopping");
                handle.stop();
            }
        })
        .context("spawning the signal thread")?;
    Ok(stopping)
}

/// Sleep for `delay`, or less if the daemon is told to stop meanwhile.
fn pause(delay: Duration, stopping: &AtomicBool) {
    let until = Instant::now() + delay;
    while !stopping.load(Ordering::SeqCst) {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        thread::sleep(left.min(STOP_POLL));
    }
}

/// What the log thread owns: the log's one writer.
struct LogThread {
    /// Where the log is, or how to find one per receiver.
    place: Place,
    cadence: Cadence,
    /// Whether the receiver's diagnostic log may be erased once held.
    adopt_log: bool,
    /// Every snapshot, lossless.
    writes: Receiver<Snapshot>,
    /// What other threads ask to have written.
    inbox: Receiver<LogRequest>,
    /// How the journal reaches the receiver: the request queue.
    journal_handle: Handle,
    journal_info: server::SharedInfo,
    /// Set once the device task has stopped, so nothing more arrives.
    finished: Arc<AtomicBool>,
}

/// Write the log until the device task has stopped.
fn record(log_thread: LogThread) {
    let LogThread {
        place,
        cadence: log_cadence,
        adopt_log,
        writes,
        inbox,
        journal_handle,
        journal_info,
        finished: log_finished,
    } = log_thread;
    let mut journal = Journal::default();
    if adopt_log {
        journal = journal.clearing_when_full();
    }
    let mut recorder = Recorder::new(place, log_cadence);
    // Soon after startup rather than immediately: the first
    // pass wants a receiver that has answered, and the errors
    // worth catching are the ones raised as it comes up.
    let mut next_journal = Instant::now() + FIRST_JOURNAL;
    // Commands are recorded on the same thread as snapshots so
    // one connection stays one writer.  Waiting on whichever
    // arrives first, rather than draining commands only when a
    // snapshot happens to publish: an audit entry should not
    // sit until the next poll, and the queue should not be lost
    // when the snapshots stop.
    loop {
        while let Ok(request) = inbox.try_recv() {
            recorder.serve(request);
        }
        if Instant::now() >= next_journal {
            // Only while a receiver is answering.  Every one of
            // these queries would otherwise wait out its
            // timeout, and the audit entries behind them would
            // wait with it.
            let attached = journal_handle
                .latest()
                .is_some_and(|s| s.freshness != Freshness::Disconnected);
            let attachment = journal_handle.attachment();
            let (identity, dialect) = {
                let current = server::lock_or_poisoned(&journal_info);
                (current.identity.clone(), current.dialect)
            };
            // The journal's rows are read from the receiver
            // attached now, so they are filed under it, not
            // under whichever unit the last snapshot came from.
            recorder.note(&identity);
            let settled = recorder.noted.as_ref() == Some(&identity);
            if let (true, true, Some(log)) = (attached, settled, recorder.log_mut())
                && let Some(receiver) = log.current_receiver()
            {
                record_strays(&journal_handle, &identity, log);
                journal.pass(
                    &journal_handle,
                    dialect,
                    receiver,
                    attachment,
                    &identity,
                    log,
                );
            }
            // From the end of the pass, not its start.  A pass
            // can run longer than the interval, and timed from
            // its start the next would already be due.
            next_journal = Instant::now() + JOURNAL_EVERY;
        }
        // Finished, or every publisher has gone.  Write what is
        // left and stop: an audit entry for a command that ran
        // is the one record of it.
        let snapshots = if log_finished.load(Ordering::SeqCst) {
            None
        } else {
            queued(&writes, AUDIT_POLL)
        };
        let Some(snapshots) = snapshots else {
            for snapshot in writes.try_iter() {
                recorder.write(&snapshot);
            }
            while let Ok(request) = inbox.try_recv() {
                recorder.serve(request);
            }
            // Errors read off the queue since the last pass, which
            // would otherwise go with the process.
            let identity = server::lock_or_poisoned(&journal_info).identity.clone();
            recorder.note(&identity);
            if recorder.noted.as_ref() == Some(&identity)
                && let Some(log) = recorder.log_mut()
            {
                record_strays(&journal_handle, &identity, log);
            }
            return;
        };
        for snapshot in snapshots {
            recorder.write(&snapshot);
        }
    }
}

/// Everything the supervisor runs on, for as long as the daemon does.
struct Supervisor {
    cli: Cli,
    baud: BaudRate,
    cadence: Cadence,
    /// Outlives every connection to the receiver.
    shared: Shared,
    /// Both ends of the request queue, which also outlives them.
    requests_tx: Sender<Request>,
    requests_rx: Receiver<Request>,
    info: server::SharedInfo,
    /// Set once the daemon has been told to stop.
    stopping: Arc<AtomicBool>,
}

/// Keep a receiver open, reopening it whenever the link dies, until
/// told to stop.
///
/// The request queue outlives any one connection.  A reconnect builds a
/// new task around a freshly opened receiver but hands it the same
/// receiver end, or every client holding a handle would be writing to a
/// channel nobody reads.
fn supervise(supervisor: Supervisor) -> Result<()> {
    let Supervisor {
        cli,
        baud,
        cadence,
        shared,
        requests_tx,
        requests_rx,
        info,
        stopping,
    } = supervisor;
    let settings = Settings {
        path: cli.device.clone(),
        baud,
        framing: cli.framing,
        read_timeout: Duration::from_millis(250),
    };
    let mut requests = requests_rx;
    let mut serving = false;
    loop {
        // Checked here as well as by the task: a stop that arrives
        // while no task is running is discarded with the queue.
        if stopping.load(Ordering::SeqCst) {
            return Ok(());
        }
        let device = match open(&settings) {
            Ok(device) => device,
            Err(e) => {
                eprintln!("smartclockd: cannot open {}: {e}", cli.device);
                shared.mark_disconnected(Timestamp::now(), &e.to_string());
                pause(RECONNECT_DELAY, &stopping);
                continue;
            }
        };

        let identity = device.identity().to_string();
        eprintln!("smartclockd: attached to {identity} on {}", cli.device);

        // Refreshed on every open, so clients are told about the
        // receiver that is actually attached and their commands are
        // gated against its command table.
        {
            let mut current = server::lock_or_poisoned(&info);
            current.identity = identity.clone();
            current.dialect = device.dialect();
            // Named here, before the socket opens, rather than when the
            // log thread gets round to opening it: a client asking on
            // connection is told the file its history will be in.
            if cli.database.is_none() {
                current.database = cli
                    .log_dir
                    .join(db::file_name(&identity, &cli.device))
                    .display()
                    .to_string();
            }
        }

        // The socket opens only once a receiver has answered, so a
        // client never connects to a daemon with nothing to say.  Later
        // reconnects reuse the listener already running.
        if !serving {
            start_server(Listening {
                socket: &cli.socket,
                shared: &shared,
                requests: &requests_tx,
                info: Arc::clone(&info),
            })?;
            serving = true;
        }

        let mut task = DeviceTask::new(device, cadence.clone(), shared.clone(), requests);
        let stopped = task.run();
        requests = task.into_requests();

        match stopped {
            Stopped::HandlesDropped => return Ok(()),
            Stopped::Requested => {
                task::discard_queued(&requests, "the daemon is stopping");
                return Ok(());
            }
            Stopped::LinkFailed(e) => {
                eprintln!("smartclockd: link failed, reopening: {e}");
                shared.mark_disconnected(Timestamp::now(), &e.to_string());
                // A command is for the receiver that was attached when
                // it was sent, so anything queued is answered rather
                // than run against whatever comes back.
                let dropped =
                    task::discard_queued(&requests, "the link went down before the command ran");
                if dropped > 0 {
                    eprintln!("smartclockd: dropped {dropped} queued commands");
                }
                pause(RECONNECT_DELAY, &stopping);
                // And again: the queue is still open during the sleep,
                // so anything submitted in that window would otherwise
                // be the first thing run against the receiver that
                // comes back -- which, on a by-id path, may not even be
                // the same unit.
                let late =
                    task::discard_queued(&requests, "the link was down when the command was sent");
                if late > 0 {
                    eprintln!("smartclockd: dropped {late} commands sent during the outage");
                }
            }
            // Reopened at once: the link is up, and the open reads the
            // new unit's identity and files it under its own serial.
            Stopped::Swapped(e) => {
                eprintln!("smartclockd: {e}; reopening");
                shared.mark_disconnected(Timestamp::now(), &e.to_string());
                let dropped = task::discard_queued(
                    &requests,
                    "the receiver was swapped before the command ran",
                );
                if dropped > 0 {
                    eprintln!("smartclockd: dropped {dropped} queued commands");
                }
            }
        }
    }
}

/// Write down a change in the receiver's alarm.
///
/// The wording distinguishes a fault going away from never having been
/// there.  "Cleared at the receiver" says something the daemon did not
/// do -- it never clears the alarm -- and saying that about the first
/// observation of a quiet receiver would be a small lie about how it
/// got that way.
fn record_alarm(
    log: &mut db::Log,
    alarm: smartclock::types::AlarmCondition,
    previous: Option<smartclock::types::AlarmCondition>,
) {
    let named = alarm.named_bits();
    let decoded = if !named.is_empty() {
        named.join(", ")
    } else if previous.is_some_and(|p| !p.is_clear()) {
        "cleared at the receiver".to_owned()
    } else {
        "clear".to_owned()
    };
    match log.record_event("alarm", alarm.bits(), &decoded) {
        Ok(()) => eprintln!("smartclockd: receiver alarm now {decoded}"),
        Err(e) => eprintln!("smartclockd: could not record an alarm change: {e}"),
    }
}

/// Journal the errors a failure read off the receiver before the drain
/// could, which are gone from its queue and exist nowhere else.
///
/// Only those raised by `attached`, the receiver the log is filing
/// under now; one kept from a unit since detached cannot be filed under
/// it any more, and is said out loud instead.
fn record_strays(handle: &Handle, attached: &str, log: &mut db::Log) {
    for (from, entry) in handle.take_stray_errors() {
        if from != attached {
            eprintln!(
                "smartclockd: not journalling {} {} from {from}, which is no longer attached",
                entry.code, entry.message
            );
            continue;
        }
        if let Err(e) = log.record_error(entry.code, &entry.message) {
            eprintln!("smartclockd: could not record an error: {e}");
        }
        if entry.is_overflow() {
            eprintln!("{LOST_TO_OVERFLOW}");
        }
    }
}

/// Every snapshot waiting to be written, after up to `wait` for the
/// first; `None` once every publisher has gone.
///
/// All of them rather than one: a journal pass can take longer than
/// snapshots take to arrive, and writing one per pass let the queue
/// fill and drop readings.
fn queued<T>(writes: &Receiver<T>, wait: Duration) -> Option<Vec<T>> {
    match writes.recv_timeout(wait) {
        Ok(first) => Some(std::iter::once(first).chain(writes.try_iter()).collect()),
        Err(RecvTimeoutError::Timeout) => Some(Vec::new()),
        Err(RecvTimeoutError::Disconnected) => None,
    }
}

/// What a log holds, said at startup.
///
/// A log holding two units' history is a thing to know before reading
/// any trend out of it, so it is said here rather than left to be
/// discovered in SQL.
fn describe(log: &db::Log, path: &Path) -> Result<()> {
    let (errors, entries, events) = log.journal_counts()?;
    let receivers = log.receivers()?;
    if receivers.len() > 1 {
        eprintln!(
            "smartclockd: this log holds history from {} receivers: {}",
            receivers.len(),
            receivers
                .iter()
                .map(|(serial, model)| format!("{model} {serial}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    eprintln!(
        "smartclockd: log at {} holds {} snapshots, {} commands, \
         {errors} receiver errors, {entries} diagnostic log entries \
         and {events} register events",
        path.display(),
        log.count()?,
        log.audit_count()?
    );
    Ok(())
}

/// Where the log thread's rows go.
enum Place {
    /// One file for everything, open from the start.
    Fixed(Option<db::Log>),
    /// A file per receiver in `dir`, opened once a receiver has
    /// answered `*IDN?`; `device` names the file of one that will not
    /// parse.
    PerReceiver { dir: PathBuf, device: String },
}

/// The log thread's writer: the database, and what it has to remember
/// between rows to file them correctly.
struct Recorder {
    place: Place,
    /// The file open now, in the per-receiver case: `None` until a
    /// receiver has answered, which is a real state -- nothing is
    /// written before there is a unit to file it under.
    log: Option<db::Log>,
    /// Which file `log` is, so a swap to a different unit is a change
    /// of file and a reconnection to the same one is not.
    opened: Option<PathBuf>,
    /// Written into each log as it is opened, for readers of it.
    cadence: Cadence,
    /// The identity the log last noted, so it is noted again only on a
    /// change.
    noted: Option<String>,
    /// The alarm as last written down, so only changes are recorded.
    /// A row per poll would be the snapshot table again; a row per
    /// change is the history worth reading.
    last_alarm: Option<smartclock::types::AlarmCondition>,
    /// A log that would not open, and when it was last tried.  Retried
    /// no more often than [`LOG_RETRY`], and reported once rather than
    /// with every snapshot.
    failed_open: Option<(PathBuf, Instant)>,
    /// When the last snapshot written was taken.  A failed poll
    /// republishes the snapshot before it, unchanged, so that clients
    /// see its values go stale; written again, it would be a second
    /// row for the same moment, counted twice by every reader.
    last_at: Option<jiff::Timestamp>,
    /// Whether `last_alarm` has been read from the database for the
    /// noted receiver.
    ///
    /// Seeded on first use rather than starting empty, or every restart
    /// writes a row saying the alarm is what it already was -- two of
    /// them showed up in one evening's upgrades.  What matters is
    /// whether it has changed since it was last recorded, which
    /// outlives this process.
    alarm_seeded: bool,
}

impl Recorder {
    fn new(mut place: Place, cadence: Cadence) -> Self {
        let log = match &mut place {
            Place::Fixed(log) => log.take(),
            Place::PerReceiver { .. } => None,
        };
        let mut recorder = Self {
            place,
            log: None,
            opened: None,
            cadence,
            noted: None,
            last_alarm: None,
            last_at: None,
            failed_open: None,
            alarm_seeded: false,
        };
        if let Some(log) = log {
            recorder.adopt(log);
        }
        recorder
    }

    /// A recorder over one open log, for tests.
    #[cfg(test)]
    fn fixed(log: db::Log) -> Self {
        let mut recorder = Self {
            place: Place::Fixed(None),
            log: None,
            opened: None,
            cadence: Cadence::default(),
            noted: None,
            last_alarm: None,
            last_at: None,
            failed_open: None,
            alarm_seeded: false,
        };
        recorder.adopt(log);
        recorder
    }

    fn log_mut(&mut self) -> Option<&mut db::Log> {
        self.log.as_mut()
    }

    /// Take a freshly opened log as the one being written.
    ///
    /// A reader judges whether a slower tier's value is still current
    /// by the cadence, and has only the database to ask.
    fn adopt(&mut self, mut log: db::Log) {
        if let Err(e) = log.note_cadence(&self.cadence) {
            eprintln!("smartclockd: could not record the cadence: {e}");
        }
        self.log = Some(log);
        self.noted = None;
        self.alarm_seeded = false;
    }

    /// Open the file `identity` belongs in, if it is not the one open.
    ///
    /// A file that cannot be opened is said out loud and left; the
    /// daemon runs on without a log rather than exiting, since a
    /// different receiver's file may open fine.
    fn place(&mut self, identity: &str) {
        let Place::PerReceiver { dir, device } = &self.place else {
            return;
        };
        let path = dir.join(db::file_name(identity, device));
        if self.opened.as_ref() == Some(&path) {
            return;
        }
        let retrying = match &self.failed_open {
            Some((failed, at)) if *failed == path => {
                if at.elapsed() < LOG_RETRY {
                    return;
                }
                true
            }
            _ => false,
        };
        match db::Log::open(&path) {
            Ok(log) => {
                if retrying {
                    eprintln!("smartclockd: {} opened; logging {identity}", path.display());
                }
                if let Err(e) = describe(&log, &path) {
                    eprintln!("smartclockd: could not read {}: {e:#}", path.display());
                }
                self.adopt(log);
                self.opened = Some(path);
                self.failed_open = None;
            }
            Err(e) => {
                if !retrying {
                    eprintln!(
                        "smartclockd: {e:#}; not logging {identity}, trying again every {} s",
                        LOG_RETRY.as_secs()
                    );
                }
                self.log = None;
                self.opened = None;
                self.noted = None;
                self.failed_open = Some((path, Instant::now()));
            }
        }
    }

    /// Make sure the log knows which receiver rows are from, so that
    /// what is written next is filed under it.
    ///
    /// Cheap enough to call before every write: a string compare, and
    /// the database is touched only when the identity has changed.
    /// Not marked noted on failure, so the next row tries again; until
    /// then rows carry no receiver and the journal waits.
    fn note(&mut self, identity: &str) {
        if identity.is_empty() || self.noted.as_deref() == Some(identity) {
            return;
        }
        self.place(identity);
        let Some(log) = self.log.as_mut() else {
            return;
        };
        match log.note_receiver(identity) {
            Ok(others) => {
                if !others.is_empty() {
                    eprintln!(
                        "smartclockd: this log also holds rows from {}; \
                         every row says which receiver it came from",
                        others.join(", ")
                    );
                }
                self.noted = Some(identity.to_owned());
                // Another receiver's alarm says nothing about whether
                // this one's has changed.
                self.alarm_seeded = false;
            }
            Err(e) => eprintln!("smartclockd: could not note the receiver: {e}"),
        }
    }

    /// Do what another thread asked.
    fn serve(&mut self, request: LogRequest) {
        match request {
            LogRequest::Audit(entry) => self.audit(&entry),
            // Logged here as well as answered, since a client told its
            // write was queued is no longer waiting to hear.
            LogRequest::Note(note, written) => {
                let outcome = self.write_note(&note);
                if let Err(why) = &outcome {
                    eprintln!("smartclockd: a note was not written: {why}");
                }
                let _ = written.send(outcome);
            }
            LogRequest::Fact(fact, written) => {
                let outcome = self.write_fact(&fact);
                if let Err(why) = &outcome {
                    eprintln!("smartclockd: a fact was not written: {why}");
                }
                let _ = written.send(outcome);
            }
        }
    }

    /// Write a note under the receiver attached when it was sent.
    fn write_note(&mut self, note: &Note) -> Result<(), String> {
        let log = self.log_for(&note.receiver)?;
        log.note(note.at, &note.text).map_err(|e| format!("{e:#}"))
    }

    /// Write a fact under the receiver attached when it was sent.
    fn write_fact(&mut self, fact: &Fact) -> Result<(), String> {
        let log = self.log_for(&fact.receiver)?;
        log.fact(fact.since, &fact.key, &fact.value)
            .map_err(|e| format!("{e:#}"))
    }

    /// The log with `receiver` noted as its current unit, or why there
    /// is none: a row written without a receiver is shown nowhere.
    fn log_for(&mut self, receiver: &str) -> Result<&mut db::Log, String> {
        self.note(receiver);
        if self.noted.as_deref() != Some(receiver) {
            return Err(format!("{receiver} could not be recorded in the log"));
        }
        let log = self.log.as_mut().ok_or("no log is open")?;
        if log.current_receiver().is_none() {
            return Err(format!("{receiver} could not be recorded in the log"));
        }
        Ok(log)
    }

    /// Write one audited command, under the receiver it was sent to
    /// and at the time it ran.
    fn audit(&mut self, entry: &Entry) {
        self.note(&entry.receiver);
        let Some(log) = self.log.as_mut() else {
            return;
        };
        if let Err(e) = log.audit(entry.at, &entry.scpi, &entry.class, &entry.outcome) {
            eprintln!("smartclockd: could not record a command: {e}");
        }
    }

    /// Write one snapshot, under the receiver it was read from.
    fn write(&mut self, snapshot: &Snapshot) {
        // Only record readings that describe the receiver.  A
        // disconnected snapshot is the same values again with a flag,
        // and logging those would pad the history with rows that look
        // like measurements.
        if snapshot.freshness == Freshness::Disconnected {
            return;
        }
        if self.last_at == Some(snapshot.at) {
            return;
        }
        // By the snapshot's own receiver, not the one attached now: a
        // snapshot can wait in the queue across a swap.
        if let Some(identity) = &snapshot.receiver {
            self.note(identity);
        }
        let Some(log) = self.log.as_mut() else {
            return;
        };
        if let Some(alarm) = snapshot.alarm {
            if !self.alarm_seeded
                && let Some(receiver) = log.current_receiver()
            {
                match log.last_alarm(receiver) {
                    Ok(bits) => {
                        self.last_alarm = bits.map(smartclock::types::AlarmCondition::from_bits);
                    }
                    Err(e) => eprintln!("smartclockd: could not read the last alarm: {e}"),
                }
                self.alarm_seeded = true;
            }
            if snapshot.alarm != self.last_alarm {
                record_alarm(log, alarm, self.last_alarm);
                self.last_alarm = snapshot.alarm;
            }
        }
        match log.record(snapshot) {
            Ok(_) => self.last_at = Some(snapshot.at),
            Err(e) => eprintln!("smartclockd: could not record a snapshot: {e}"),
        }
    }
}

/// Give the socket owner and group access, and nobody else.
#[cfg(unix)]
fn set_socket_mode(socket: &Path) -> Result<()> {
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o660))
        .with_context(|| format!("setting permissions on {}", socket.display()))
}

/// Elsewhere the platform decides; there is no portable equivalent.
#[cfg(not(unix))]
fn set_socket_mode(_socket: &Path) -> Result<()> {
    Ok(())
}

/// How often a log that would not open is tried again.  A failure that
/// lasts -- a permission, a full disk, a newer schema -- would otherwise
/// be retried and reported with every snapshot, once a second.
const LOG_RETRY: Duration = Duration::from_secs(60);

/// The longest cadence worth accepting.
///
/// A day between polls is already useless; past this the arithmetic is
/// the problem rather than the setting.  `Duration::from_secs_f64`
/// panics outright on a large enough value, and a merely absurd one
/// overflows the `Instant` the schedule advances.
const MAX_CADENCE_SECONDS: f64 = 86_400.0;

/// A cadence flag as a duration, refusing what cannot be one.
fn seconds(value: f64, flag: &str) -> Result<Duration> {
    if !value.is_finite() || value <= 0.0 {
        anyhow::bail!("{flag} must be a positive number of seconds, not {value}");
    }
    if value > MAX_CADENCE_SECONDS {
        anyhow::bail!("{flag} must be at most {MAX_CADENCE_SECONDS} seconds, not {value}");
    }
    Ok(Duration::from_secs_f64(value))
}

/// Open the receiver and identify it, at the configured line settings
/// or, when it does not answer at those, at the ones it does.
///
/// The path may name a serial port or a receiver on the network;
/// `transport::open` decides which, so every tool accepts the same
/// paths.
fn open(settings: &Settings) -> Result<Device<Box<dyn Transport + Send>>> {
    let attached = attach(settings, &Config::default()).context("identifying the receiver")?;
    if let Some(report) = attached.probe_report(settings) {
        eprintln!("smartclockd: {report}");
    }
    Ok(attached.device)
}

/// What the socket server needs to start.
struct Listening<'a> {
    /// Where to bind.
    socket: &'a Path,
    /// State to serve to clients.
    shared: &'a Shared,
    /// Where client commands go.
    requests: &'a Sender<Request>,
    /// What to tell clients about the receiver and the policy.
    info: server::SharedInfo,
}

fn start_server(listening: Listening<'_>) -> Result<()> {
    let socket = listening.socket;
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let name = socket
        .to_fs_name::<GenericFilePath>()
        .context("naming the socket")?
        .into_owned();
    // A socket left behind by a crash would otherwise block the bind,
    // so it is removed -- but only if nothing answers on it.  Removing a
    // live daemon's socket left that daemon running, logging and
    // unreachable, and its clients reconnecting to this one.
    if Stream::connect(name.clone()).is_ok() {
        anyhow::bail!(
            "another daemon is serving {}; stop it first",
            socket.display()
        );
    }
    let _ = std::fs::remove_file(socket);
    // Bound here, not in the serving thread, so a failure is reported
    // to whoever started the daemon rather than to a dying thread.
    let listener = server::bind(name)?;
    // Socket permissions are the whole of the authorization model, so
    // they are set rather than inherited from whatever umask the daemon
    // happened to start with.  Group access is deliberate: it is how an
    // unprivileged operator runs the monitor.
    set_socket_mode(socket)?;
    let handle = Handle::new(listening.requests.clone(), listening.shared.clone());
    let info = listening.info;
    let where_to = socket.display().to_string();
    thread::Builder::new()
        .name("smartclockd-socket".to_owned())
        .spawn(move || {
            if let Err(e) = server::serve(listener, handle, info) {
                eprintln!("smartclockd: socket server stopped: {e}");
            }
        })
        .context("spawning the socket server")?;
    eprintln!("smartclockd: listening on {where_to}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Entry;
    use super::Fact;
    use super::Listening;
    use super::LogRequest;
    use super::Note;
    use super::Place;
    use super::Recorder;
    use super::db;
    use super::queued;
    use super::record_strays;
    use super::server;
    use super::start_server;
    use crate::scratch::Scratch;
    use smartclock::device::Device;
    use smartclock::session::Config;
    use smartclock::session::Session;
    use smartclock::snapshot::Freshness;
    use smartclock::snapshot::Snapshot;
    use smartclock::task;
    use smartclock::task::Cadence;
    use smartclock::task::Shared;
    use smartclock::types::AlarmCondition;
    use smartclock_sim::receiver::Receiver;
    use smartclock_sim::transport::SimTransport;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    #[test]
    fn everything_queued_behind_a_slow_pass_is_taken_at_once() {
        let (tx, rx) = channel();
        for n in 0..100 {
            tx.send(n).expect("queue");
        }
        assert_eq!(queued(&rx, Duration::ZERO), Some((0..100).collect()));
        assert_eq!(queued(&rx, Duration::ZERO), Some(Vec::new()));
        drop(tx);
        assert_eq!(queued::<i32>(&rx, Duration::ZERO), None);
    }

    #[test]
    fn each_receiver_gets_its_own_file_and_a_swap_switches_files() {
        let dir = std::env::temp_dir().join(format!("smartclockd-perunit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let mut recorder = Recorder {
            place: Place::PerReceiver {
                dir: dir.clone(),
                device: "/dev/ttyUSB0".to_owned(),
            },
            log: None,
            opened: None,
            cadence: Cadence::default(),
            noted: None,
            last_alarm: None,
            last_at: None,
            failed_open: None,
            alarm_seeded: false,
        };
        let snapshot = |identity: &str| {
            let mut snapshot = Snapshot::new(jiff::Timestamp::now());
            snapshot.receiver = Some(identity.to_owned());
            snapshot.freshness = Freshness::Live;
            snapshot
        };
        // Nothing is open until a unit has answered.
        assert!(recorder.log.is_none());
        recorder.write(&snapshot("HEWLETT-PACKARD,58503A,3710A01056,3704-C"));
        assert_eq!(
            recorder.opened.as_deref(),
            Some(dir.join("58503A-3710A01056.sqlite").as_path())
        );
        recorder.write(&snapshot("SYMMETRICOM,Z3805A,3625A01487,3944-A"));
        assert_eq!(
            recorder.opened.as_deref(),
            Some(dir.join("Z3805A-3625A01487.sqlite").as_path())
        );
        recorder.write(&snapshot("HEWLETT-PACKARD,58503A,3710A01056,3704-C"));
        let again = snapshot("HEWLETT-PACKARD,58503A,3710A01056,3704-C");
        recorder.write(&again);
        // A failed poll republishes the last snapshot as it was: the
        // same moment is not written twice.
        recorder.write(&again);
        // Each file holds only its own unit's rows, and knows one unit.
        for (name, rows) in [
            ("58503A-3710A01056.sqlite", 3),
            ("Z3805A-3625A01487.sqlite", 1),
        ] {
            let log = db::Log::open(&dir.join(name)).expect("reopen");
            assert_eq!(log.count().expect("count"), rows, "{name}");
            assert_eq!(log.receivers().expect("receivers").len(), 1, "{name}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_error_a_failed_command_read_first_is_journalled() {
        let mut simulated = Receiver::default();
        simulated.queue_error(-313, "Calibration memory lost");
        let identity = simulated.identity.clone();
        let device = Device::open(Session::new(
            SimTransport::new(simulated),
            Config::default(),
        ))
        .expect("open the simulated receiver");
        let (handle, _joiner) = task::spawn(device, Cadence::default());
        // Refused while locked; explaining the refusal reads the queue,
        // and the -313 ahead of it comes off with it.
        assert!(
            handle
                .request(":SYNChronization:HOLDover:TUNCertainty:PRESent?")
                .is_err()
        );

        let scratch = Scratch::new("strays");
        let path = scratch.path();
        let mut log = db::Log::open(path).expect("open the database");
        log.note_receiver(&identity).expect("note the receiver");
        record_strays(&handle, &identity, &mut log);
        let (errors, _, _) = log.journal_counts().expect("count");
        assert_eq!(errors, 1);
    }

    #[test]
    fn each_receiver_is_compared_with_its_own_last_alarm() {
        let scratch = Scratch::new("alarms");
        let path = scratch.path();
        let mut recorder = Recorder::fixed(db::Log::open(path).expect("open the database"));
        let mut write = |serial: &str, bits: u16| {
            let mut snapshot = Snapshot::new(jiff::Timestamp::now());
            snapshot.freshness = Freshness::Live;
            snapshot.receiver = Some(format!("HEWLETT-PACKARD,58503A,{serial},3704-C"));
            snapshot.alarm = Some(AlarmCondition::from_bits(bits));
            recorder.write(&snapshot);
        };
        // B asserted, then A clear, then B clear: B's clear is a change
        // for B, whatever A last said.
        write("B", 1 << 3);
        write("A", 0);
        write("B", 0);

        let b = recorder
            .log
            .as_ref()
            .expect("a log")
            .current_receiver()
            .expect("B noted");
        let last = recorder
            .log
            .as_ref()
            .expect("a log")
            .last_alarm(b)
            .expect("read the alarm");
        assert_eq!(last, Some(0));
    }

    #[test]
    fn a_command_is_audited_when_and_where_it_ran() {
        let scratch = Scratch::new("audit");
        let path = scratch.path();
        let mut recorder = Recorder::fixed(db::Log::open(path).expect("open the database"));
        // Written after a swap, and after a journal pass kept the
        // writer busy: neither may change the row.
        recorder.note("HEWLETT-PACKARD,58503A,B,3704-C");
        let ran: jiff::Timestamp = "2026-09-23T12:00:00Z".parse().expect("a time");
        recorder.audit(&Entry {
            at: ran,
            receiver: "HEWLETT-PACKARD,58503A,A,3704-C".to_owned(),
            scpi: ":SYNChronization:HOLDover:INITiate".to_owned(),
            class: "control".to_owned(),
            outcome: "ok".to_owned(),
        });
        let rows = recorder
            .log
            .as_ref()
            .expect("a log")
            .audit_rows()
            .expect("read");
        assert_eq!(
            rows,
            vec![(
                "2026-09-23T12:00:00.000000000Z".to_owned(),
                Some("A".to_owned())
            )]
        );
    }

    #[test]
    fn a_note_and_a_fact_are_written_under_the_unit_attached_when_sent() {
        let scratch = Scratch::new("notes");
        let mut recorder =
            Recorder::fixed(db::Log::open(scratch.path()).expect("open the database"));
        recorder.note("HEWLETT-PACKARD,58503A,B,3704-C");
        let at: jiff::Timestamp = "2026-09-25T14:00:00Z".parse().expect("a time");
        let (written, outcome) = channel();
        recorder.serve(LogRequest::Note(
            Note {
                at,
                receiver: "HEWLETT-PACKARD,58503A,A,3704-C".to_owned(),
                text: "added a 20 dB LNA".to_owned(),
            },
            written.clone(),
        ));
        recorder.serve(LogRequest::Fact(
            Fact {
                since: at,
                receiver: "HEWLETT-PACKARD,58503A,A,3704-C".to_owned(),
                key: "ocxo.model".to_owned(),
                value: "10811-60159".to_owned(),
            },
            written,
        ));
        assert_eq!(outcome.try_iter().collect::<Vec<_>>(), vec![Ok(()), Ok(())]);
        let log = recorder.log.as_ref().expect("a log");
        let a = Some("A".to_owned());
        assert_eq!(
            log.note_rows().expect("read"),
            vec![
                ("added a 20 dB LNA".to_owned(), a.clone()),
                ("ocxo.model = 10811-60159".to_owned(), a),
            ]
        );
        assert_eq!(
            log.fact_rows().expect("read"),
            vec![("ocxo.model".to_owned(), "10811-60159".to_owned())]
        );
    }

    #[test]
    fn a_snapshot_queued_across_a_swap_is_filed_under_the_unit_it_came_from() {
        let scratch = Scratch::new("swap");
        let path = scratch.path();
        let mut recorder = Recorder::fixed(db::Log::open(path).expect("open the database"));
        // The journal has already noted the new unit when a snapshot
        // read from the old one comes off the queue.
        recorder.note("HEWLETT-PACKARD,58503A,B,3704-C");
        let mut snapshot = Snapshot::new(jiff::Timestamp::now());
        snapshot.freshness = Freshness::Live;
        snapshot.receiver = Some("HEWLETT-PACKARD,58503A,A,3704-C".to_owned());
        recorder.write(&snapshot);

        let serials = recorder
            .log
            .as_ref()
            .expect("a log")
            .snapshot_serials()
            .expect("read the rows");
        assert_eq!(serials, vec![Some("A".to_owned())]);
    }

    #[test]
    fn a_live_socket_is_not_taken_over_and_a_stale_one_is() {
        let socket =
            std::env::temp_dir().join(format!("smartclockd-takeover-{}.sock", std::process::id()));
        // Left behind by a crash: not a listening socket at all.
        std::fs::write(&socket, b"").expect("a stale file");
        let shared = Shared::new();
        let (requests, _queue) = channel();
        let (audit, _audit) = channel();
        let info = || {
            std::sync::Arc::new(std::sync::Mutex::new(server::Info {
                identity: String::new(),
                dialect: smartclock::command::Dialect::Hp58503,
                database: String::new(),
                policy: server::Policy::default(),
                inbox: crate::inbox::LogInbox::new(audit.clone()),
                cadence: Cadence::default(),
            }))
        };
        let listening = |info| Listening {
            socket: &socket,
            shared: &shared,
            requests: &requests,
            info,
        };
        start_server(listening(info())).expect("the stale file is replaced");
        let refused = start_server(listening(info())).expect_err("the socket is live");
        assert!(format!("{refused}").contains("another daemon"), "{refused}");
        let _ = std::fs::remove_file(&socket);
    }

    #[test]
    fn a_log_that_will_not_open_is_not_retried_with_every_snapshot() {
        // A directory that is a file: nothing can be created inside it.
        let dir =
            std::env::temp_dir().join(format!("smartclockd-unopenable-{}", std::process::id()));
        std::fs::write(&dir, b"").expect("a file in the way");
        let mut recorder = Recorder::new(
            Place::PerReceiver {
                dir: dir.clone(),
                device: "/dev/ttyUSB0".to_owned(),
            },
            Cadence::default(),
        );
        let snapshot = || {
            let mut snapshot = Snapshot::new(jiff::Timestamp::now());
            snapshot.receiver = Some("HEWLETT-PACKARD,58503A,3710A01056,3704-C".to_owned());
            snapshot.freshness = Freshness::Live;
            snapshot
        };
        recorder.write(&snapshot());
        let first = recorder.failed_open.as_ref().map(|(_, at)| *at);
        assert!(first.is_some(), "the failure was not remembered");
        recorder.write(&snapshot());
        assert_eq!(
            recorder.failed_open.as_ref().map(|(_, at)| *at),
            first,
            "tried again within the retry interval"
        );
        let _ = std::fs::remove_file(&dir);
    }
}
