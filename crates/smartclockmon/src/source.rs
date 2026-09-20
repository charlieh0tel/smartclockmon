//! Where snapshots come from.
//!
//! Normally the daemon, which owns the serial port.  Direct mode opens
//! the receiver itself, which is convenient before the daemon is
//! installed but records no history, so the display says which is in
//! use rather than letting the two look alike.

use std::io::BufRead;
use std::io::BufReader;
use std::sync::mpsc::Receiver;
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
            Self::Daemon { socket, .. } => format!("daemon {socket}"),
            Self::Direct { device } => {
                // A by-id path is the whole name, and is far too long
                // for a header; the tail identifies it well enough.
                let short = device.rsplit('/').next().unwrap_or(device);
                let short = match short
                    .char_indices()
                    .nth(short.chars().count().saturating_sub(28))
                {
                    Some((at, _)) if short.chars().count() > 28 => format!("...{}", &short[at..]),
                    _ => short.to_owned(),
                };
                format!("direct {short} (not logging)")
            }
        }
    }
}

/// Connect to a running daemon and stream its snapshots.
pub(crate) fn from_daemon(socket: &str) -> Result<(Receiver<Snapshot>, Attachment)> {
    let name = socket
        .to_fs_name::<GenericFilePath>()
        .context("naming the socket")?;
    let stream = Stream::connect(name)
        .with_context(|| format!("connecting to {socket}; is smartclockd running?"))?;

    let (tx, rx) = channel();
    let reader = BufReader::new(stream);
    thread::Builder::new()
        .name("smartclockmon-reader".to_owned())
        .spawn(move || {
            for line in reader.lines() {
                let Ok(line) = line else { return };
                // Replies to requests share the stream with snapshots;
                // the monitor only watches, so anything without a
                // snapshot is not its business.
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                let Some(snapshot) = value.get("snapshot") else {
                    continue;
                };
                match serde_json::from_value::<Snapshot>(snapshot.clone()) {
                    Ok(snapshot) => {
                        if tx.send(snapshot).is_err() {
                            return;
                        }
                    }
                    Err(_) => continue,
                }
            }
        })
        .context("spawning the reader thread")?;

    Ok((
        rx,
        Attachment::Daemon {
            socket: socket.to_owned(),
            database: None,
        },
    ))
}

/// Open the receiver directly and poll it in this process.
pub(crate) fn from_device(device: &str, baud: u32) -> Result<(Receiver<Snapshot>, Attachment)> {
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
    let updates = handle.subscribe();
    // The handle must outlive this call or the task stops, so it is
    // leaked deliberately: the monitor polls until the process ends.
    std::mem::forget(handle);

    Ok((
        updates,
        Attachment::Direct {
            device: device.to_owned(),
        },
    ))
}
