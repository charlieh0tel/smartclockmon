//! Logging daemon for one SmartClock receiver.
//!
//! Holds the serial port open for as long as it runs, so EFC and
//! holdover history keeps accumulating whether or not anyone is
//! watching.  Clients reach it over a local socket for live state, and
//! by opening the SQLite log read-only for history.

mod audit;

mod db;
mod journal;
mod server;

use crate::audit::Audit;
use crate::journal::Journal;
use crate::server::Policy;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
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
use interprocess::local_socket::ToFsName as _;
use jiff::Timestamp;
use smartclock::command::Dialect;
use smartclock::device::Device;
use smartclock::session::Config;
use smartclock::session::Session;
use smartclock::snapshot::Freshness;
use smartclock::snapshot::Snapshot;
use smartclock::task;
use smartclock::task::Cadence;
use smartclock::task::DeviceTask;
use smartclock::task::Handle;
use smartclock::task::Request;
use smartclock::task::Shared;
use smartclock::task::Stopped;
use smartclock::transport;
use smartclock::transport::Transport;
use smartclock::transport::serial::Settings;
use smartclock::types::BaudRate;

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

    /// Where to keep the snapshot log.
    #[arg(
        long,
        env = "SMARTCLOCKD_DATABASE",
        default_value = "/var/lib/smartclockd/snapshots.sqlite"
    )]
    database: PathBuf,

    /// Where to listen for clients.
    #[arg(
        long,
        env = "SMARTCLOCKD_SOCKET",
        default_value = "/run/smartclockd/socket"
    )]
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

/// How long after startup the receiver's own records are first read.
const FIRST_JOURNAL: Duration = Duration::from_secs(5);

/// How often they are read after that.
///
/// Matched to the medium tier rather than to the minute it used to be,
/// because reading the event registers is now part of a pass and an
/// event register is the only thing that catches a transition between
/// two condition polls.  Reading them a minute apart would collapse a
/// minute of transitions into one bitmask and lose the timing that is
/// the whole reason to read them.
///
/// The cost when nothing has happened is seven short queries: five
/// event registers, the error queue, and the diagnostic log count.  The
/// error queue and the log answer in one reply each when empty and
/// unchanged, which is nearly always.
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

    if let Some(parent) = cli.database.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    // A database this binary cannot read is a configuration mistake,
    // not a transient failure, so it exits like one: retrying cannot
    // turn a newer schema into an older one, and Restart= would
    // otherwise reopen it every five seconds forever.
    let log = match db::Log::open(&cli.database) {
        Ok(log) => log,
        Err(e) => {
            eprintln!("smartclockd: {e:#}");
            std::process::exit(CONFIGURATION_ERROR);
        }
    };
    // First line in the journal, so "which build is running" is
    // answerable from the logs alone rather than by finding the binary.
    eprintln!("smartclockd: version {}", smartclock::VERSION);
    let (errors, entries, events) = log.journal_counts()?;
    // Said at startup rather than left to be discovered in SQL: a log
    // holding two units' history is a thing to know before reading any
    // trend out of it.
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
        cli.database.display(),
        log.count()?,
        log.audit_count()?
    );

    let (audit_tx, audit_rx) = channel::<crate::audit::Entry>();

    let shared = Shared::new();
    let (requests_tx, requests_rx) = channel();

    // from_secs_f64 panics on a negative or non-finite value, so a
    // typo in a flag would abort rather than being reported.
    let cadence = Cadence {
        fast: seconds(cli.fast, "--fast")?,
        medium: seconds(cli.medium, "--medium")?,
        slow: seconds(cli.slow, "--slow")?,
    };
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
    let database = cli.database.display().to_string();
    let info: server::SharedInfo = Arc::new(Mutex::new(server::Info {
        identity: String::new(),
        dialect: Dialect::Hp58503,
        database: database.clone(),
        policy,
        audit: Audit::new(audit_tx),
        cadence: cadence.clone(),
        generation: 0,
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
    thread::Builder::new()
        .name("smartclockd-log".to_owned())
        .spawn(move || {
            let mut journal = Journal::default();
            if adopt_log {
                journal = journal.clearing_when_full();
            }
            let mut recorder = Recorder::new(log);
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
                while let Ok(entry) = audit_rx.try_recv() {
                    if let Err(e) =
                        recorder
                            .log
                            .audit(&entry.scpi, &entry.class, &entry.outcome, None)
                    {
                        eprintln!("smartclockd: could not record a command: {e}");
                    }
                }
                if Instant::now() >= next_journal {
                    // Only while a receiver is answering.  Every one of
                    // these queries would otherwise wait out its
                    // timeout, and the audit entries behind them would
                    // wait with it.
                    let attached = journal_handle
                        .latest()
                        .is_some_and(|s| s.freshness != Freshness::Disconnected);
                    let (identity, dialect, generation) = {
                        let current = server::lock_or_poisoned(&journal_info);
                        (
                            current.identity.clone(),
                            current.dialect,
                            current.generation,
                        )
                    };
                    // The journal's rows are read from the receiver
                    // attached now, so they are filed under it, not
                    // under whichever unit the last snapshot came from.
                    recorder.note(&identity);
                    let settled = recorder.noted.as_ref() == Some(&identity);
                    if let (true, true, Some(receiver)) =
                        (attached, settled, recorder.log.current_receiver())
                    {
                        journal.pass(
                            &journal_handle,
                            dialect,
                            receiver,
                            generation,
                            &mut recorder.log,
                        );
                    }
                    // From the end of the pass, not its start.  A pass
                    // can run longer than the interval, and timed from
                    // its start the next would already be due.
                    next_journal = Instant::now() + JOURNAL_EVERY;
                }
                // Every publisher has gone, which only happens when the
                // daemon is shutting down.  Write what is left and stop.
                let Some(snapshots) = queued(&writes, AUDIT_POLL) else {
                    while let Ok(entry) = audit_rx.try_recv() {
                        let _ = recorder
                            .log
                            .audit(&entry.scpi, &entry.class, &entry.outcome, None);
                    }
                    return;
                };
                for snapshot in snapshots {
                    recorder.write(&snapshot);
                }
            }
        })
        .context("spawning the log thread")?;

    supervise(cli, baud, cadence, shared, requests_tx, requests_rx, info)
}

/// Keep a receiver open, reopening it whenever the link dies.
///
/// The request queue outlives any one connection.  A reconnect builds a
/// new task around a freshly opened receiver but hands it the same
/// receiver end, or every client holding a handle would be writing to a
/// channel nobody reads.
fn supervise(
    cli: Cli,
    baud: BaudRate,
    cadence: Cadence,
    shared: Shared,
    requests_tx: Sender<Request>,
    requests_rx: Receiver<Request>,
    info: server::SharedInfo,
) -> Result<()> {
    let settings = Settings {
        path: cli.device.clone(),
        baud,
        read_timeout: Duration::from_millis(250),
    };
    let mut requests = requests_rx;
    let mut serving = false;
    loop {
        let device = match open(&settings) {
            Ok(device) => device,
            Err(e) => {
                eprintln!("smartclockd: cannot open {}: {e}", cli.device);
                shared.mark_disconnected(Timestamp::now(), &e.to_string());
                thread::sleep(RECONNECT_DELAY);
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
            current.generation += 1;
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
                thread::sleep(RECONNECT_DELAY);
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

/// The log thread's writer: the database, and what it has to remember
/// between rows to file them correctly.
struct Recorder {
    log: db::Log,
    /// The identity the log last noted, so it is noted again only on a
    /// change.
    noted: Option<String>,
    /// The alarm as last written down, so only changes are recorded.
    /// A row per poll would be the snapshot table again; a row per
    /// change is the history worth reading.
    last_alarm: Option<smartclock::types::AlarmCondition>,
    /// Whether `last_alarm` has been read from the database.
    ///
    /// Seeded on first use rather than starting empty, or every restart
    /// writes a row saying the alarm is what it already was -- two of
    /// them showed up in one evening's upgrades.  What matters is
    /// whether it has changed since it was last recorded, which
    /// outlives this process.
    alarm_seeded: bool,
}

impl Recorder {
    fn new(log: db::Log) -> Self {
        Self {
            log,
            noted: None,
            last_alarm: None,
            alarm_seeded: false,
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
        match self.log.note_receiver(identity) {
            Ok(others) => {
                if !others.is_empty() {
                    eprintln!(
                        "smartclockd: this log also holds rows from {}; \
                         every row says which receiver it came from",
                        others.join(", ")
                    );
                }
                self.noted = Some(identity.to_owned());
            }
            Err(e) => eprintln!("smartclockd: could not note the receiver: {e}"),
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
        // By the snapshot's own receiver, not the one attached now: a
        // snapshot can wait in the queue across a swap.
        if let Some(identity) = &snapshot.receiver {
            self.note(identity);
        }
        if let Some(alarm) = snapshot.alarm {
            if !self.alarm_seeded
                && let Some(receiver) = self.log.current_receiver()
            {
                match self.log.last_alarm(receiver) {
                    Ok(bits) => {
                        self.last_alarm = bits.map(smartclock::types::AlarmCondition::from_bits);
                    }
                    Err(e) => eprintln!("smartclockd: could not read the last alarm: {e}"),
                }
                self.alarm_seeded = true;
            }
            if snapshot.alarm != self.last_alarm {
                record_alarm(&mut self.log, alarm, self.last_alarm);
                self.last_alarm = snapshot.alarm;
            }
        }
        if let Err(e) = self.log.record(snapshot) {
            eprintln!("smartclockd: could not record a snapshot: {e}");
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

/// Open the receiver and identify it.
///
/// The path may name a serial port or a receiver on the network;
/// `transport::open` decides which, so every tool accepts the same
/// paths.
fn open(settings: &Settings) -> Result<Device<Box<dyn Transport + Send>>> {
    let port =
        transport::open(settings).with_context(|| format!("cannot open {}", settings.path))?;
    let session = Session::new(port, Config::default());
    Device::open(session).context("identifying the receiver")
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
    // A socket left behind by a crash would otherwise block the bind.
    let _ = std::fs::remove_file(socket);

    let name = socket
        .to_fs_name::<GenericFilePath>()
        .context("naming the socket")?
        .into_owned();
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
    use super::Recorder;
    use super::db;
    use super::queued;
    use smartclock::snapshot::Freshness;
    use smartclock::snapshot::Snapshot;
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
    fn a_snapshot_queued_across_a_swap_is_filed_under_the_unit_it_came_from() {
        let path = std::env::temp_dir().join(format!("smartclockd-swap-{}.db", std::process::id()));
        let wipe = || {
            for suffix in ["", "-wal", "-shm"] {
                let mut name = path.clone().into_os_string();
                name.push(suffix);
                let _ = std::fs::remove_file(name);
            }
        };
        wipe();
        let mut recorder = Recorder::new(db::Log::open(&path).expect("open the database"));
        // The journal has already noted the new unit when a snapshot
        // read from the old one comes off the queue.
        recorder.note("HEWLETT-PACKARD,58503A,B,3704-C");
        let mut snapshot = Snapshot::new(jiff::Timestamp::now());
        snapshot.freshness = Freshness::Live;
        snapshot.receiver = Some("HEWLETT-PACKARD,58503A,A,3704-C".to_owned());
        recorder.write(&snapshot);

        let serials = recorder.log.snapshot_serials().expect("read the rows");
        drop(recorder);
        wipe();
        assert_eq!(serials, vec![Some("A".to_owned())]);
    }
}
