//! Logging daemon for one SmartClock receiver.
//!
//! Holds the serial port open for as long as it runs, so EFC and
//! holdover history keeps accumulating whether or not anyone is
//! watching.  Clients reach it over a local socket for live state, and
//! by opening the SQLite log read-only for history.

mod audit;

mod db;
mod proto;
mod server;

use crate::audit::Audit;
use crate::server::Policy;

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use std::sync::mpsc::channel;
use std::thread;
use std::time::Duration;

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
use smartclock::task::Cadence;
use smartclock::task::DeviceTask;
use smartclock::task::Handle;
use smartclock::task::Request;
use smartclock::task::Shared;
use smartclock::task::Stopped;
use smartclock::transport::Transport;
use smartclock::transport::serial::SerialTransport;
use smartclock::transport::serial::Settings;
use smartclock::transport::tcp::TcpTransport;
use smartclock::types::BaudRate;

#[derive(Parser)]
#[command(about, version)]
struct Cli {
    /// Serial device, or `tcp://host:port` for a receiver on the
    /// network or the simulator.  Prefer a /dev/serial/by-id/... path
    /// for a local one; /dev/ttyUSB0 does not survive re-enumeration.
    #[arg(long, env = "SMARTCLOCKD_DEVICE", default_value = "/dev/ttyUSB0")]
    device: String,

    /// Bits per second.  Checked against the four the receiver accepts.
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

    /// Seconds between status screen polls.  The screen is about 1.8 KB,
    /// close to a second of wire time at 19200, so this cannot be small.
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

    /// Permit raw SCPI the command table does not recognise.  An
    /// unrecognised command is treated as control, or as dangerous if
    /// it resembles one that can strand the link.
    #[arg(long, env = "SMARTCLOCKD_ALLOW_RAW")]
    allow_raw: bool,
}

/// How often the log thread looks for a command to record when no
/// snapshot has arrived to wake it.
const AUDIT_POLL: Duration = Duration::from_millis(200);

/// How long to wait before reopening a receiver that went away.
const RECONNECT_DELAY: Duration = Duration::from_secs(5);

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
    let mut log = db::Log::open(&cli.database)?;
    eprintln!(
        "smartclockd: log at {} holds {} snapshots and {} commands",
        cli.database.display(),
        log.count()?,
        log.audit_count()?
    );

    let (audit_tx, audit_rx) = channel::<crate::audit::Entry>();

    let shared = Shared::new();
    let (requests_tx, requests_rx) = channel();

    // Writing the log is a subscriber, so a slow or failing write
    // cannot hold up the receiver -- but not an ordinary one.  An
    // ordinary subscriber that falls behind is dropped, and for the
    // thread that writes the history that means logging stops for good
    // while the daemon runs on saying nothing.
    let writes = shared.subscribe_lossless();
    let database = cli.database.display().to_string();
    thread::Builder::new()
        .name("smartclockd-log".to_owned())
        .spawn(move || {
            // Commands are recorded on the same thread as snapshots so
            // one connection stays one writer.  Waiting on whichever
            // arrives first, rather than draining commands only when a
            // snapshot happens to publish: an audit entry should not
            // sit until the next poll, and the queue should not be lost
            // when the snapshots stop.
            loop {
                while let Ok(entry) = audit_rx.try_recv() {
                    if let Err(e) = log.audit(&entry.scpi, &entry.class, &entry.outcome, None) {
                        eprintln!("smartclockd: could not record a command: {e}");
                    }
                }
                let snapshot = match writes.recv_timeout(AUDIT_POLL) {
                    Ok(snapshot) => snapshot,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    // Every publisher has gone, which only happens
                    // when the daemon is shutting down.  Write what is
                    // left and stop.
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        while let Ok(entry) = audit_rx.try_recv() {
                            let _ = log.audit(&entry.scpi, &entry.class, &entry.outcome, None);
                        }
                        return;
                    }
                };
                // Only record readings that describe the receiver.  A
                // disconnected snapshot is the same values again with a
                // flag, and logging those would pad the history with
                // rows that look like measurements.
                if snapshot.freshness == Freshness::Disconnected {
                    continue;
                }
                if let Err(e) = log.record(&snapshot) {
                    eprintln!("smartclockd: could not record a snapshot: {e}");
                }
            }
        })
        .context("spawning the log thread")?;

    supervise(
        cli,
        baud,
        shared,
        requests_tx,
        requests_rx,
        database,
        Audit::new(audit_tx),
    )
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
    shared: Shared,
    requests_tx: Sender<Request>,
    requests_rx: Receiver<Request>,
    database: String,
    audit: Audit,
) -> Result<()> {
    let settings = Settings {
        path: cli.device.clone(),
        baud,
        read_timeout: Duration::from_millis(250),
    };
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

    let info: server::SharedInfo = Arc::new(Mutex::new(server::Info {
        identity: String::new(),
        dialect: Dialect::Hp58503,
        database: database.clone(),
        policy,
        audit,
    }));

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

        let identity = format!(
            "{},{},{},{}",
            device.identity().manufacturer,
            device.identity().model,
            device.identity().serial,
            device.identity().firmware
        );
        eprintln!("smartclockd: attached to {identity} on {}", cli.device);

        // Refreshed on every open, so clients are told about the
        // receiver that is actually attached and their commands are
        // gated against its command table.
        match info.lock() {
            Ok(mut current) => {
                current.identity = identity.clone();
                current.dialect = device.dialect();
            }
            Err(poisoned) => {
                let mut current = poisoned.into_inner();
                current.identity = identity.clone();
                current.dialect = device.dialect();
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
            Stopped::LinkFailed(e) => {
                eprintln!("smartclockd: link failed, reopening: {e}");
                shared.mark_disconnected(Timestamp::now(), &e.to_string());
                // A command is for the receiver that was attached when
                // it was sent, so anything queued is answered rather
                // than run against whatever comes back.
                let dropped = DeviceTask::<SerialTransport>::discard_queued(
                    &requests,
                    "the link went down before the command ran",
                );
                if dropped > 0 {
                    eprintln!("smartclockd: dropped {dropped} queued commands");
                }
                thread::sleep(RECONNECT_DELAY);
                // And again: the queue is still open during the sleep,
                // so anything submitted in that window would otherwise
                // be the first thing run against the receiver that
                // comes back -- which, on a by-id path, may not even be
                // the same unit.
                let late = DeviceTask::<SerialTransport>::discard_queued(
                    &requests,
                    "the link was down when the command was sent",
                );
                if late > 0 {
                    eprintln!("smartclockd: dropped {late} commands sent during the outage");
                }
            }
        }
    }
}

/// Give the socket owner and group access, and nobody else.
#[cfg(unix)]
fn set_socket_mode(socket: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
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

/// Open whatever the device path names.
///
/// A `tcp://host:port` path reaches a receiver over the network, which
/// covers both a serial adapter behind ser2net and the simulator.  The
/// simulator listens on TCP rather than offering a pseudo-terminal,
/// since a PTY would confine it to Unix.
fn open(settings: &Settings) -> Result<Device<Box<dyn Transport + Send>>> {
    let port: Box<dyn Transport + Send> = match settings.path.strip_prefix("tcp://") {
        Some(address) => Box::new(
            TcpTransport::connect(address, settings.read_timeout)
                .with_context(|| format!("connecting to {address}"))?,
        ),
        None => Box::new(
            SerialTransport::open(settings)
                .with_context(|| format!("opening {} at {}", settings.path, settings.baud))?,
        ),
    };
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
