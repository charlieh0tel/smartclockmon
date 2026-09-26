//! Where snapshots come from.
//!
//! Normally the daemon, which owns the serial port.  Direct mode opens
//! the receiver itself, which is convenient before the daemon is
//! installed but records no history, so the display says which is in
//! use rather than letting the two look alike.

use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;
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
use smartclock::task;
use smartclock::task::Cadence;
use smartclock::task::Handle;
use smartclock::transport;
use smartclock::transport::serial::Settings;
use smartclock::types::BaudRate;
use smartclock::types::Framing;
use smartclock::wire::Reading;

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
    Reading(Box<Reading>),
    /// An answer to something the console sent.
    Reply(String),
    /// The source went away.  The monitor keeps the last values on
    /// screen but must stop presenting them as current.
    Lost(String),
    /// The daemon was reached again, and says this about itself.
    ///
    /// It may be a different daemon on the same socket, started with a
    /// different log, policy or cadence, so what the first connection
    /// said is not assumed to hold.
    Reattached {
        /// Where it keeps its log.
        database: Option<String>,
        /// What it lets a client do.
        policy: Policy,
        /// How often it polls.
        cadence: Cadence,
    },
}

/// What the daemon says a client may do.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Policy {
    /// Whether control commands are permitted.
    pub(crate) control: bool,
    /// Whether commands that can strand the link are permitted.
    pub(crate) dangerous: bool,
    /// Whether unrecognised SCPI is passed through.
    pub(crate) raw: bool,
}

/// Sends commands to the daemon.
///
/// The send half has to be kept: dropping it, as an earlier version
/// did, leaves a monitor that can only listen.
#[derive(Debug, Clone, Default)]
pub(crate) struct Console {
    /// The current connection's write half, replaced on reconnect.
    ///
    /// Held behind a shared slot rather than by value because the
    /// reader thread reconnects on its own: keeping the half from the
    /// first connection left the console writing to a closed socket
    /// after any daemon restart, silently, with readings still
    /// arriving on the new one.
    writer: Arc<Mutex<Option<SendHalf>>>,
    /// The receiver itself, in direct mode, where there is no daemon.
    ///
    /// Held here also because the task stops when its last handle
    /// goes, and the console lives as long as the monitor.
    device: Option<Handle>,
}

impl Console {
    /// Whether there is anywhere to send.
    pub(crate) fn is_connected(&self) -> bool {
        self.writer.lock().is_ok_and(|writer| writer.is_some())
    }

    /// Point the console at a new connection.
    fn attach(&self, send: SendHalf) {
        if let Ok(mut writer) = self.writer.lock() {
            *writer = Some(send);
        }
    }

    /// Ask the daemon to read one status screen.
    ///
    /// The snapshot carrying it arrives on the subscription like any
    /// other, so nothing waits here.  The reply itself is ignored: the
    /// reader forwards only what the console asked for.
    ///
    /// No tier polls the screen -- it costs the receiver about 1.5 s of
    /// its link -- so this is what a sky plot is made of, and it is
    /// sent only while someone is looking at one.
    pub(crate) fn sky(&self) -> Result<()> {
        // Direct mode asks the task, which delivers the screen to
        // subscribers the same way.  On a thread of its own because the
        // read takes about 1.5 s and the caller is drawing the screen.
        if let Some(device) = &self.device {
            let device = device.clone();
            thread::Builder::new()
                .name("smartclockmon-sky".to_owned())
                .spawn(move || {
                    let _ = device.sky();
                })?;
            return Ok(());
        }
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow::anyhow!("poisoned writer"))?;
        let Some(writer) = writer.as_mut() else {
            return Err(anyhow::anyhow!("not connected to a daemon"));
        };
        writeln!(writer, r#"{{"v":1,"id":"sky","op":{{"kind":"sky"}}}}"#)?;
        writer.flush()?;
        Ok(())
    }

    /// Send one command.  The answer arrives as an [`Update::Reply`].
    pub(crate) fn send(&self, scpi: &str) -> Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow::anyhow!("poisoned writer"))?;
        let Some(writer) = writer.as_mut() else {
            return Err(anyhow::anyhow!("not connected to a daemon"));
        };
        let request = serde_json::json!({
            "v": 1,
            "id": "console",
            "op": { "kind": "query", "scpi": scpi },
        });
        writeln!(writer, "{request}")?;
        writer.flush()?;
        Ok(())
    }
}

/// The write half of a connection to the daemon.
type SendHalf = <Stream as interprocess::local_socket::traits::Stream>::SendHalf;

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
pub(crate) fn from_daemon(
    socket: &str,
) -> Result<(Receiver<Update>, Attachment, Console, Policy, Cadence)> {
    // Ask where the log lives before streaming starts, so the history
    // panes can open it without being told the path separately and
    // without risking a mismatch with the daemon's own.  The reader is
    // handed on rather than rebuilt: it has already buffered whatever
    // snapshots arrived alongside the reply.
    let (first, send, database, policy, cadence) = connect_and_ask(socket)
        .with_context(|| format!("connecting to {socket}; is smartclockd running?"))?;
    let console = Console::default();
    console.attach(send);

    let (tx, rx) = channel();
    let path = socket.to_owned();
    let reconnected = console.clone();
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
                            Ok((open, send, database, policy, cadence)) => {
                                // The console follows the link, or it
                                // would keep writing to the socket that
                                // just died.
                                reconnected.attach(send);
                                let told = Update::Reattached {
                                    database,
                                    policy,
                                    cadence,
                                };
                                if tx.send(told).is_err() {
                                    return;
                                }
                                stream = Some(open);
                            }
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
        console,
        policy,
        cadence,
    ))
}

/// Connect, ask where the log lives, and return the reader with the
/// answer.
///
/// The reply shares the stream with snapshots, so lines that are not it
/// are forwarded rather than dropped: the reader is returned still
/// holding them.
fn connect_and_ask(socket: &str) -> Result<(Reader, SendHalf, Option<String>, Policy, Cadence)> {
    let mut stream = connect(socket)?;
    writeln!(stream, r#"{{"v":1,"id":"info","op":{{"kind":"info"}}}}"#)?;
    stream.flush()?;

    let (recv, send) = stream.split();
    let mut reader = BufReader::new(recv);
    let mut database = None;
    let mut policy = Policy::default();
    // Defaults only until the daemon says otherwise; it may have been
    // started with any cadence, and a pane that calls data stale needs
    // the real one to know what late means.
    let mut cadence = Cadence::default();
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
            let flag = |name: &str| {
                value
                    .pointer(&format!("/ok/{name}"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
            };
            policy = Policy {
                control: flag("allow_control"),
                dangerous: flag("allow_dangerous"),
                raw: flag("allow_raw"),
            };
            // An older daemon does not report these, so each falls back
            // to the default rather than to zero.
            let seconds = |name: &str, fallback: Duration| {
                value
                    .pointer(&format!("/ok/{name}"))
                    .and_then(serde_json::Value::as_f64)
                    .filter(|s| s.is_finite() && *s > 0.0)
                    .map_or(fallback, Duration::from_secs_f64)
            };
            let default = Cadence::default();
            cadence = Cadence {
                fast: seconds("cadence_fast", default.fast),
                medium: seconds("cadence_medium", default.medium),
                slow: seconds("cadence_slow", default.slow),
            };
            break;
        }
    }
    Ok((reader, send, database, policy, cadence))
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
            // Not a snapshot, so it is an answer to something the
            // console asked.
            if value.get("id").and_then(serde_json::Value::as_str) == Some("console") {
                let answer = match (value.pointer("/ok/lines"), value.get("err")) {
                    (Some(lines), _) => lines
                        .as_array()
                        .map(|l| {
                            l.iter()
                                .filter_map(serde_json::Value::as_str)
                                .collect::<Vec<_>>()
                                .join(" | ")
                        })
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "ok".to_owned()),
                    (None, Some(err)) => {
                        format!("error: {}", err.as_str().unwrap_or_default())
                    }
                    _ => continue,
                };
                tx.send(Update::Reply(answer)).map_err(|_| ())?;
            }
            continue;
        };
        let Ok(snapshot) = serde_json::from_value::<Reading>(snapshot.clone()) else {
            continue;
        };
        tx.send(Update::Reading(Box::new(snapshot)))
            .map_err(|_| ())?;
    }
    Ok(())
}

/// Open the receiver directly and poll it in this process.
pub(crate) fn from_device(
    device: &str,
    baud: u32,
    framing: Framing,
) -> Result<(Receiver<Update>, Attachment, Console, Policy, Cadence)> {
    let baud = BaudRate::new(baud).with_context(|| {
        let supported = BaudRate::ALL.map(|b| b.to_string()).join(", ");
        format!("{baud} is not a rate the receiver supports ({supported})")
    })?;
    let settings = Settings {
        path: device.to_owned(),
        baud,
        framing,
        read_timeout: Duration::from_millis(250),
    };
    // Through the same opener as the other tools, so `tcp://host:port`
    // reaches a network bridge or the simulator here as it does there.
    let port = transport::open(&settings)
        .with_context(|| format!("opening {device}; is smartclockd holding it?"))?;
    let receiver =
        Device::open(Session::new(port, Config::default())).context("identifying the receiver")?;

    let cadence = Cadence::default();
    let (handle, _joiner) = task::spawn(receiver, cadence.clone());
    let readings = handle.subscribe();

    // Direct mode publishes Snapshots; wrap them so both sources look
    // the same to the monitor.
    let (tx, rx) = channel();
    thread::Builder::new()
        .name("smartclockmon-direct".to_owned())
        .spawn(move || {
            for snapshot in readings {
                // Direct mode polls the receiver itself, so it holds a
                // Snapshot; converting keeps one shape on screen.
                let reading = Reading::from(&snapshot);
                if tx.send(Update::Reading(Box::new(reading))).is_err() {
                    return;
                }
            }
            let _ = tx.send(Update::Lost("the device task stopped".to_owned()));
        })
        .context("spawning the direct reader")?;

    // Direct mode has no daemon to ask, so the console has nowhere to
    // send commands and says so rather than appearing to work.  It can
    // still ask the task for a status screen.
    let console = Console {
        device: Some(handle),
        ..Console::default()
    };
    Ok((
        rx,
        Attachment::Direct {
            device: device.to_owned(),
        },
        console,
        Policy::default(),
        cadence,
    ))
}

#[cfg(test)]
mod tests {
    use super::Update;
    use super::from_device;
    use smartclock::types::Framing;
    use smartclock_sim::net::serve;
    use smartclock_sim::receiver::Receiver;
    use smartclock_sim::transport::SimTransport;
    use std::net::TcpListener;
    use std::time::Duration;

    /// A simulated receiver on a loopback port, for one connection.
    fn simulator() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("an address");
        std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                let _ = serve(stream, SimTransport::new(Receiver::default()));
            }
        });
        format!("tcp://{address}")
    }

    #[test]
    fn direct_mode_reads_the_sky_when_asked() {
        let (updates, _, console, ..) =
            from_device(&simulator(), 9600, Framing::default()).expect("open");
        console.sky().expect("ask for the sky");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match updates.recv_timeout(left) {
                Ok(Update::Reading(reading)) if reading.screen.is_some() => return,
                Ok(_) => {}
                Err(e) => panic!("no sky arrived: {e}"),
            }
        }
    }

    #[test]
    fn direct_mode_opens_a_receiver_by_network_address() {
        // The console is kept: it holds the task's handle, and the task
        // stops when that goes.
        let (updates, _, _console, ..) =
            from_device(&simulator(), 9600, Framing::default()).expect("open by address");
        match updates.recv_timeout(Duration::from_secs(10)) {
            Ok(Update::Reading(_)) => {}
            other => panic!("expected a reading, got {other:?}"),
        }
    }
}
