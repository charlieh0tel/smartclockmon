//! Where snapshots come from.
//!
//! Normally the daemon, which owns the serial port.  Direct mode opens
//! the receiver itself, which is convenient before the daemon is
//! installed but records no history, so the display says which is in
//! use rather than letting the two look alike.

use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use std::sync::mpsc::channel;
use std::thread;
use std::time::Duration;

use anyhow::Context as _;
use anyhow::Result;
use interprocess::local_socket::GenericFilePath;
use interprocess::local_socket::Stream;
use interprocess::local_socket::ToFsName as _;
use interprocess::local_socket::traits::Stream as _;
use smartclock::device::Device;
use smartclock::session::Config;
use smartclock::session::Session;
use smartclock::snapshot::Snapshot;
use smartclock::task;
use smartclock::task::Cadence;
use smartclock::transport::serial::SerialTransport;
use smartclock::transport::serial::Settings;
use smartclock::types::BaudRate;

/// How the monitor is attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Attachment {
    /// Reading the daemon's stream.  History is being recorded.
    Daemon {
        /// Socket path.
        socket: String,
        /// Where the daemon keeps its log, from its info reply.
        database: Option<String>,
    },
    /// Talking to the receiver directly.  Nothing is being recorded.
    Direct {
        /// Device path.
        device: String,
    },
}

impl Attachment {
    /// A short description for the header.
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Daemon { socket, .. } => format!("daemon {}", tail(socket, 40)),
            // Saying so matters: direct mode looks identical to daemon
            // mode on screen but records nothing.
            Self::Direct { device } => format!("direct {} (not logging)", tail(device, 28)),
        }
    }
}

/// The last `keep` characters of a path, marked as shortened.
///
/// Both a by-id device name and a socket under a long state directory
/// run past any sensible header width, and the tail is the part that
/// identifies them.
fn tail(path: &str, keep: usize) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let count = name.chars().count();
    if count <= keep {
        return name.to_owned();
    }
    let skip = count - keep;
    format!("...{}", name.chars().skip(skip).collect::<String>())
}

/// Something the monitor should react to.
#[derive(Debug)]
pub(crate) enum Update {
    /// A new reading.
    Reading(Box<Snapshot>),
    /// The source went away.  The monitor keeps the last values on
    /// screen but must stop presenting them as current.
    Lost(String),
}

/// How long to wait before trying the daemon again.
///
/// The unit restarts it, so a monitor that quit on every daemon restart
/// would be a nuisance.  Reconnecting keeps a dashboard usable across
/// an upgrade or a crash.
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

/// Connect to a running daemon and stream its snapshots.
///
/// The first connection is made here, so running the monitor with no
/// daemon says so at once rather than sitting on an empty screen.
/// Later disconnections are handled by the reader, which keeps trying.
pub(crate) fn from_daemon(socket: &str) -> Result<(Receiver<Update>, Attachment)> {
    // Ask where the log lives before streaming starts, so the history
    // panes can open it without being told the path separately and
    // without risking a mismatch with the daemon's own.  The reader is
    // handed on rather than rebuilt: it has already buffered whatever
    // snapshots arrived alongside the reply.
    let (first, database) = connect_and_ask(socket)
        .with_context(|| format!("connecting to {socket}; is smartclockd running?"))?;

    let (tx, rx) = channel();
    let path = socket.to_owned();
    thread::Builder::new()
        .name("smartclockmon-reader".to_owned())
        .spawn(move || {
            let mut stream = Some(first);
            loop {
                match stream.take() {
                    Some(open) => {
                        if forward(open, &tx).is_err() {
                            return;
                        }
                        if tx
                            .send(Update::Lost("the daemon closed the connection".to_owned()))
                            .is_err()
                        {
                            return;
                        }
                    }
                    None => {
                        thread::sleep(RECONNECT_DELAY);
                        match connect_and_ask(&path) {
                            Ok((open, _)) => stream = Some(open),
                            Err(e) => {
                                if tx.send(Update::Lost(e.to_string())).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        })
        .context("spawning the reader thread")?;

    Ok((
        rx,
        Attachment::Daemon {
            socket: socket.to_owned(),
            database,
        },
    ))
}

/// Connect, ask where the log lives, and return the reader with the
/// answer.
///
/// The reply shares the stream with snapshots, so lines that are not it
/// are forwarded rather than dropped: the reader is returned still
/// holding them.
fn connect_and_ask(socket: &str) -> Result<(Reader, Option<String>)> {
    let mut stream = connect(socket)?;
    writeln!(stream, r#"{{"v":1,"id":"info","op":{{"kind":"info"}}}}"#)?;
    stream.flush()?;

    let (recv, _send) = stream.split();
    let mut reader = BufReader::new(recv);
    let mut database = None;
    for _ in 0..MAX_LINES_BEFORE_INFO {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value.get("id").and_then(serde_json::Value::as_str) == Some("info") {
            database = value
                .pointer("/ok/database")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            break;
        }
    }
    Ok((reader, database))
}

/// The buffered read half of a connection to the daemon.
type Reader = BufReader<<Stream as interprocess::local_socket::traits::Stream>::RecvHalf>;

/// How many snapshot lines to read past while waiting for the info
/// reply.  A handful: the daemon sends the current snapshot on connect
/// and then one per poll, so the reply arrives almost immediately.
const MAX_LINES_BEFORE_INFO: usize = 8;

fn connect(socket: &str) -> Result<Stream> {
    let name = socket
        .to_fs_name::<GenericFilePath>()
        .context("naming the socket")?;
    Ok(Stream::connect(name)?)
}

/// Forward snapshots until the daemon hangs up.
///
/// `Err` means the monitor has gone, not the daemon, so the caller
/// stops rather than reconnecting to nobody.
fn forward(reader: Reader, tx: &Sender<Update>) -> Result<(), ()> {
    for line in reader.lines() {
        let Ok(line) = line else { return Ok(()) };
        // Replies to requests share the stream with snapshots; the
        // monitor only watches, so anything without a snapshot is not
        // its business.
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let Some(snapshot) = value.get("snapshot") else {
            continue;
        };
        let Ok(snapshot) = serde_json::from_value::<Snapshot>(snapshot.clone()) else {
            continue;
        };
        tx.send(Update::Reading(Box::new(snapshot)))
            .map_err(|_| ())?;
    }
    Ok(())
}

/// Open the receiver directly and poll it in this process.
pub(crate) fn from_device(device: &str, baud: u32) -> Result<(Receiver<Update>, Attachment)> {
    let baud = BaudRate::new(baud).with_context(|| {
        let supported = BaudRate::ALL.map(|b| b.to_string()).join(", ");
        format!("{baud} is not a rate the receiver supports ({supported})")
    })?;
    let settings = Settings {
        path: device.to_owned(),
        baud,
        read_timeout: Duration::from_millis(250),
    };
    let port = SerialTransport::open(&settings)
        .with_context(|| format!("opening {device}; is smartclockd holding it?"))?;
    let receiver =
        Device::open(Session::new(port, Config::default())).context("identifying the receiver")?;

    let (handle, _joiner) = task::spawn(receiver, Cadence::default());
    let readings = handle.subscribe();
    // The handle must outlive this call or the task stops, so it is
    // leaked deliberately: the monitor polls until the process ends.
    std::mem::forget(handle);

    // Direct mode publishes Snapshots; wrap them so both sources look
    // the same to the monitor.
    let (tx, rx) = channel();
    thread::Builder::new()
        .name("smartclockmon-direct".to_owned())
        .spawn(move || {
            for snapshot in readings {
                if tx.send(Update::Reading(Box::new(snapshot))).is_err() {
                    return;
                }
            }
            let _ = tx.send(Update::Lost("the device task stopped".to_owned()));
        })
        .context("spawning the direct reader")?;

    Ok((
        rx,
        Attachment::Direct {
            device: device.to_owned(),
        },
    ))
}
