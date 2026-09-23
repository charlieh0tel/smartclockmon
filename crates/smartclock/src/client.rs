//! Talking to a running daemon over its socket.
//!
//! Here rather than in each program because three of them do it: the
//! monitor, the command line tool and the exporter.  The daemon owns
//! the serial port for as long as it runs, so asking it is the only way
//! to read the receiver without stopping it.

use std::io::BufRead as _;
use std::io::BufReader;
use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

use interprocess::TryClone as _;
use interprocess::local_socket::GenericFilePath;
use interprocess::local_socket::Stream;
use interprocess::local_socket::ToFsName as _;
use interprocess::local_socket::traits::Stream as _;

use crate::error::Error;
use crate::error::Result;
use crate::protocol::Message;
use crate::protocol::Op;
use crate::protocol::Request;
use crate::protocol::VERSION;
use crate::wire::Reading;

/// How long to wait on a daemon that has stopped answering.
///
/// Generous next to the slowest thing the daemon does -- a status
/// screen is about a second of wire time -- and short next to never.
/// Without a deadline a daemon that is alive but wedged parks its
/// caller for good: the exporter would leak the thread serving each
/// scrape, one every fifteen seconds, until the process died.
const DEADLINE: Duration = Duration::from_secs(20);

/// Put a deadline on a connection.
///
/// Reaching through the enum because `interprocess`'s portable
/// `Stream` exposes no timeout of its own and no handle to set one on;
/// the Unix variant does.  Elsewhere the deadline is simply absent,
/// which is the same position this was in before.
#[cfg(unix)]
fn set_deadlines(stream: &Stream) {
    use std::os::fd::AsFd;

    let Stream::UdSocket(stream) = stream;
    let fd = stream.as_fd();
    let socket = socket2::SockRef::from(&fd);
    let _ = socket.set_read_timeout(Some(DEADLINE));
    let _ = socket.set_write_timeout(Some(DEADLINE));
}

#[cfg(not(unix))]
fn set_deadlines(_stream: &Stream) {}

/// A connection to a running daemon.
#[derive(Debug)]
pub struct Daemon {
    writer: Stream,
    reader: BufReader<Stream>,
    /// Distinguishes this client's replies from the snapshots that
    /// arrive unasked on the same stream.
    next_id: u32,
}

impl Daemon {
    /// Connect to the daemon listening on `socket`.
    pub fn connect(socket: &Path) -> Result<Self> {
        let name = socket.to_fs_name::<GenericFilePath>().map_err(|e| {
            Error::Daemon(format!(
                "{} is not a usable socket name: {e}",
                socket.display()
            ))
        })?;
        let stream = Stream::connect(name)?;
        set_deadlines(&stream);
        let reader = BufReader::new(stream.try_clone()?);
        Ok(Self {
            writer: stream,
            reader,
            next_id: 0,
        })
    }

    /// Send one request and wait for the reply that echoes its id.
    ///
    /// Snapshots arrive on the same stream whether or not anything was
    /// asked, so anything that is not this request's reply is skipped
    /// rather than mistaken for one.
    pub fn ask(&mut self, op: Op) -> Result<serde_json::Value> {
        // Wrapping rather than overflowing: a connection held open for
        // long enough would otherwise panic before sending anything,
        // and a repeated id is harmless here because a reply can only
        // arrive while its own request is outstanding.
        self.next_id = self.next_id.wrapping_add(1);
        let id = self.next_id.to_string();
        let request = Request {
            v: VERSION,
            id: id.clone(),
            op,
        };
        let line = serde_json::to_string(&request)
            .map_err(|e| Error::Daemon(format!("cannot encode a request: {e}")))?;
        writeln!(self.writer, "{line}")?;
        self.writer.flush()?;

        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => return Err(Error::Daemon("it closed the connection".to_owned())),
                Ok(_) => {}
                // The deadline expiring arrives as EAGAIN or ETIMEDOUT,
                // which as a message says only "resource temporarily
                // unavailable".  Say what actually happened.
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Err(Error::Daemon(format!(
                        "it did not answer within {}s",
                        DEADLINE.as_secs()
                    )));
                }
                Err(e) => return Err(e.into()),
            }
            // A message this version does not understand is not a reason
            // to give up on the one being waited for.
            let Ok(message) = serde_json::from_str::<Message>(&line) else {
                continue;
            };
            match message {
                Message::Reply {
                    id: got, ok, err, ..
                } if got == id => {
                    return match (ok, err) {
                        (Some(value), _) => Ok(value),
                        (None, Some(why)) => Err(Error::Daemon(why)),
                        (None, None) => {
                            Err(Error::Daemon("neither an answer nor a reason".to_owned()))
                        }
                    };
                }
                // A refusal that belongs to the connection rather than
                // to a request: the daemon sends one with an empty id
                // when it is full, precisely so that being full does
                // not look like having crashed.  Skipping it and then
                // reporting the EOF told the operator the opposite.
                Message::Reply {
                    id: got,
                    err: Some(why),
                    ..
                } if got.is_empty() => return Err(Error::Daemon(why)),
                _ => continue,
            }
        }
    }

    /// What the daemon is attached to, and what it permits.
    pub fn info(&mut self) -> Result<serde_json::Value> {
        self.ask(Op::Info)
    }

    /// The daemon's most recent reading.
    ///
    /// Costs the receiver nothing: the daemon answers from what it last
    /// polled rather than going to the wire.
    pub fn latest(&mut self) -> Result<Reading> {
        let value = self.ask(Op::Latest)?;
        serde_json::from_value(value)
            .map_err(|e| Error::Daemon(format!("its reading did not parse: {e}")))
    }

    /// Read one status screen, and the reading that carries it.
    ///
    /// Unlike `latest` this does go to the wire, and costs about 1.5 s
    /// of it: no tier polls the screen, because per-satellite
    /// elevation, azimuth and signal strength are all it still answers
    /// that nothing else does.  Ask while someone is looking at a sky
    /// plot, not on a timer.
    pub fn sky(&mut self) -> Result<Reading> {
        let value = self.ask(Op::Sky)?;
        serde_json::from_value(value)
            .map_err(|e| Error::Daemon(format!("its reading did not parse: {e}")))
    }

    /// Send one command and return the lines it answered with.
    ///
    /// What is permitted depends on the flags the daemon was started
    /// with, not on this call.
    pub fn query(&mut self, scpi: &str) -> Result<Vec<String>> {
        let value = self.ask(Op::Query {
            scpi: scpi.to_owned(),
        })?;
        Ok(value
            .get("lines")
            .and_then(|l| serde_json::from_value::<Vec<String>>(l.clone()).ok())
            .unwrap_or_default())
    }
}
