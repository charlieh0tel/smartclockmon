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
use smartclock::transport::serial::SerialTransport;
use smartclock::transport::serial::Settings;
use smartclock::types::BaudRate;

#[derive(Parser)]
#[command(about, version)]
struct Cli {
    /// Serial device.  Prefer a /dev/serial/by-id/... path, which
    /// survives USB re-enumeration; /dev/ttyUSB0 does not.
    #[arg(long, default_value = "/dev/ttyUSB0")]
    device: String,

    /// Bits per second.  Checked against the four the receiver accepts.
    #[arg(long, default_value_t = 19200)]
    baud: u32,

    /// Where to keep the snapshot log.
    #[arg(long, default_value = "/var/lib/smartclockd/snapshots.sqlite")]
    database: PathBuf,

    /// Where to listen for clients.
    #[arg(long, default_value = "/run/smartclockd/socket")]
    socket: PathBuf,

    /// Seconds between fast-tier polls.
    #[arg(long, default_value_t = 1.0)]
    fast: f64,

    /// Seconds between status screen polls.  The screen is about 1.8 KB,
    /// close to a second of wire time at 19200, so this cannot be small.
    #[arg(long, default_value_t = 10.0)]
    medium: f64,

    /// Seconds between position and date polls.
    #[arg(long, default_value_t = 60.0)]
    slow: f64,

    /// Permit commands that change receiver state: holdover, survey,
    /// antenna delay, elevation mask.
    #[arg(long)]
    allow_control: bool,

    /// Permit commands that can strand the link or wipe configuration:
    /// system preset, serial reconfiguration, flash erase, language
    /// change.  A baud change persists across power cycles.
    #[arg(long)]
    allow_dangerous: bool,

    /// Permit raw SCPI the command table does not recognise.  An
    /// unrecognised command is treated as control, or as dangerous if
    /// it resembles one that can strand the link.
    #[arg(long)]
    allow_raw: bool,
}

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

    // Writing the log is a subscriber like any other, so a slow or
    // failing write cannot hold up the receiver.
    let writes = shared.subscribe();
    let database = cli.database.display().to_string();
    thread::Builder::new()
        .name("smartclockd-log".to_owned())
        .spawn(move || {
            // Commands are recorded on the same thread as snapshots so
            // one connection stays one writer.
            loop {
                while let Ok(entry) = audit_rx.try_recv() {
                    if let Err(e) = log.audit(&entry.scpi, &entry.class, &entry.outcome, None) {
                        eprintln!("smartclockd: could not record a command: {e}");
                    }
                }
                let Ok(snapshot) = writes.recv() else { return };
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
    let cadence = Cadence {
        fast: Duration::from_secs_f64(cli.fast),
        medium: Duration::from_secs_f64(cli.medium),
        slow: Duration::from_secs_f64(cli.slow),
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

        // The socket opens only once a receiver has answered, so a
        // client never connects to a daemon with nothing to say.  Later
        // reconnects reuse the listener already running.
        if !serving {
            start_server(Listening {
                socket: &cli.socket,
                shared: &shared,
                requests: &requests_tx,
                info: server::Info {
                    identity: identity.clone(),
                    dialect: device.dialect(),
                    database: database.clone(),
                    policy,
                    audit: audit.clone(),
                },
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
                thread::sleep(RECONNECT_DELAY);
            }
        }
    }
}

fn open(settings: &Settings) -> Result<Device<SerialTransport>> {
    let port = SerialTransport::open(settings)
        .with_context(|| format!("opening {} at {}", settings.path, settings.baud))?;
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
    info: server::Info,
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
    let handle = Handle::new(listening.requests.clone(), listening.shared.clone());
    let info = listening.info;
    let where_to = socket.display().to_string();
    thread::Builder::new()
        .name("smartclockd-socket".to_owned())
        .spawn(move || {
            if let Err(e) = server::serve(name, handle, info) {
                eprintln!("smartclockd: socket server stopped: {e}");
            }
        })
        .context("spawning the socket server")?;
    eprintln!("smartclockd: listening on {where_to}");
    Ok(())
}
