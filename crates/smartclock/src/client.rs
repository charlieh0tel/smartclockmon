//! Talking to a running daemon over its socket.
//!
//! Here rather than in each program because four of them do it: the
//! monitor, the command line tool, the exporter and the web view.  The
//! daemon owns
//! the serial port for as long as it runs, so asking it is the only way
//! to read the receiver without stopping it.

use std::io::BufRead as _;
use std::io::BufReader;
use std::io::Write as _;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use jiff::Timestamp;

use crate::defaults::DAEMON;
use crate::error::Error;
use crate::error::Result;
use crate::link;
use crate::link::Stream;
use crate::parse;
use crate::parse::Identity;
use crate::protocol::Message;
use crate::protocol::Op;
use crate::protocol::Protocol;
use crate::protocol::Request;
use crate::screen::Screen;
use crate::sensors;
use crate::task::Cadence;
use crate::wire::Reading;

/// How long to wait on a daemon that has stopped answering.
///
/// Generous next to the slowest thing the daemon does -- a status
/// screen is about a second of wire time -- and short next to never.
/// Without a deadline a daemon that is alive but wedged parks its
/// caller for good: the exporter would leak the thread serving each
/// scrape, one every fifteen seconds, until the process died.
pub const DEADLINE: Duration = Duration::from_secs(20);

/// A connection to a running daemon.
#[derive(Debug)]
pub struct Daemon {
    writer: Stream,
    reader: BufReader<Stream>,
    /// Distinguishes this client's replies from the snapshots that
    /// arrive unasked on the same stream.
    next_id: u32,
    /// When the whole connection gives up, if it was opened with a
    /// budget: every request on it fails past this, however recently
    /// the daemon last said something.
    until: Option<Instant>,
}

impl Daemon {
    /// Connect to the daemon listening on `socket`.
    pub fn connect(socket: &Path) -> Result<Self> {
        Self::open(socket, None)
    }

    /// Connect, and give up on everything asked of this connection
    /// once `budget` has passed.
    ///
    /// For a caller with a deadline of its own to keep: a scrape that
    /// Prometheus abandons after ten seconds is no use answered after
    /// twenty.
    pub fn connect_within(socket: &Path, budget: Duration) -> Result<Self> {
        Self::open(socket, Some(Instant::now() + budget))
    }

    fn open(socket: &Path, until: Option<Instant>) -> Result<Self> {
        let stream = link::connect(socket)?;
        let reader = BufReader::new(stream.try_clone()?);
        Ok(Self {
            writer: stream,
            reader,
            next_id: 0,
            until,
        })
    }

    /// Send one request and wait for the reply that echoes its id.
    ///
    /// Snapshots arrive on the same stream whether or not anything was
    /// asked, so anything that is not this request's reply is skipped
    /// rather than mistaken for one.  Any service speaking this
    /// framing can be asked, in its own requests.
    pub fn ask<O: Protocol>(&mut self, op: O) -> Result<serde_json::Value> {
        // Wrapping rather than overflowing: a connection held open for
        // long enough would otherwise panic before sending anything,
        // and a repeated id is harmless here because a reply can only
        // arrive while its own request is outstanding.
        self.next_id = self.next_id.wrapping_add(1);
        let id = self.next_id.to_string();
        let request = Request {
            v: O::VERSION,
            id: id.clone(),
            op,
        };
        let line = serde_json::to_string(&request)
            .map_err(|e| Error::Daemon(format!("cannot encode a request: {e}")))?;
        // One deadline for the whole request, not one per read: the
        // daemon pushes a snapshot every second, and each restarted a
        // per-read timeout, so a request it never answered was never
        // timed out at all.
        let asked = Instant::now();
        let until = self
            .until
            .map_or(asked + DEADLINE, |budget| budget.min(asked + DEADLINE));
        let allowed = until.saturating_duration_since(asked).as_secs_f64();
        let timed_out = || Error::Daemon(format!("it did not answer within {allowed:.1}s"));
        writeln!(self.writer, "{line}")?;
        self.writer.flush()?;

        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(timed_out());
            }
            self.reader.get_ref().set_read_timeout(Some(left))?;
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
                    return Err(timed_out());
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

    /// Read one status screen.
    ///
    /// Unlike `latest` this does go to the wire, and costs about 1.5 s
    /// of it: no tier polls the screen, because per-satellite
    /// elevation, azimuth and signal strength are all it still answers
    /// that nothing else does.  Ask while someone is looking at the
    /// status view, not on a timer.
    pub fn status(&mut self) -> Result<Screen> {
        let value = self.ask(Op::Status)?;
        let screen = value
            .get("screen")
            .cloned()
            .ok_or_else(|| Error::Daemon("its reply carried no screen".to_owned()))?;
        serde_json::from_value(screen)
            .map_err(|e| Error::Daemon(format!("its screen did not parse: {e}")))
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

    /// Write a note to the attached receiver's log.  Nothing is sent to
    /// the receiver.
    pub fn note(&mut self, text: &str, at: Option<Timestamp>) -> Result<Filed> {
        let value = self.ask(Op::Note {
            text: text.to_owned(),
            at,
        })?;
        filed_under(&value)
    }

    /// Record a fact about the attached receiver.  Nothing is sent to
    /// the receiver.
    pub fn fact(&mut self, key: &str, value: &str, since: Option<Timestamp>) -> Result<Filed> {
        let value = self.ask(Op::Fact {
            key: key.to_owned(),
            value: value.to_owned(),
            since,
        })?;
        filed_under(&value)
    }

    /// The sensor service's account of itself, from a connection to its
    /// socket.
    pub fn sensor_info(&mut self) -> Result<sensors::Info> {
        let value = self.ask(sensors::Op::SensorInfo)?;
        serde_json::from_value(value)
            .map_err(|e| Error::Daemon(format!("its account of itself did not parse: {e}")))
    }

    /// Every sensor and its latest reading, from a connection to the
    /// sensor service's socket.
    pub fn sensor_readings(&mut self) -> Result<sensors::Latest> {
        let value = self.ask(sensors::Op::SensorLatest)?;
        serde_json::from_value(value)
            .map_err(|e| Error::Daemon(format!("its readings did not parse: {e}")))
    }
}

/// Where a note or fact went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filed {
    /// The `*IDN?` of the receiver it was filed under.
    pub receiver: String,
    /// Whether it is in the log yet; if not, it is queued behind the
    /// daemon's own work and will be written shortly.
    pub written: bool,
}

/// Where a note or fact went, from the daemon's reply.
fn filed_under(reply: &serde_json::Value) -> Result<Filed> {
    let receiver = reply
        .get("receiver")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::Daemon("its reply named no receiver".to_owned()))?;
    Ok(Filed {
        receiver: receiver.to_owned(),
        written: reply
            .get("written")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
    })
}

/// Where the daemons a collector asks are.
#[derive(Debug, Clone)]
pub enum Daemons {
    /// The run directory: one instance per subdirectory, its socket
    /// inside, `<dir>/<instance>/socket`.
    Dir(PathBuf),
    /// Named outright.
    Listed(Vec<Instance>),
}

/// Refuses a run directory, for a command line where there are no Unix
/// sockets for one to hold.
pub fn no_run_dir(_: &str) -> std::result::Result<PathBuf, String> {
    Err(
        "there are no Unix sockets here, so no run directory; name the daemons with --daemon"
            .to_owned(),
    )
}

/// One daemon a collector asks.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Instance {
    /// The instance name: its subdirectory's, or as it was named.
    pub name: String,
    /// Where it is, as [`Daemon::connect`] takes it.
    pub endpoint: PathBuf,
}

impl Instance {
    /// One daemon as a command line names it, `[NAME=]ENDPOINT`.
    ///
    /// Unnamed, it goes by the endpoint as written, so that every one a
    /// collector lists has a name of its own.
    pub fn named(text: &str) -> Self {
        match text.split_once('=') {
            Some((name, endpoint)) if !name.is_empty() && !name.contains(['/', '\\']) => Self {
                name: name.to_owned(),
                endpoint: PathBuf::from(endpoint),
            },
            _ => Self {
                name: text.to_owned(),
                endpoint: PathBuf::from(text),
            },
        }
    }
}

impl Daemons {
    /// The daemons `named`, each `[NAME=]ENDPOINT`; when none is,
    /// every one under `run_dir`; without one of those either, the one
    /// at [`DAEMON`], if there is one.
    pub fn from_names(named: &[String], run_dir: Option<&Path>) -> Self {
        match run_dir {
            _ if !named.is_empty() => {
                Self::Listed(named.iter().map(|text| Instance::named(text)).collect())
            }
            Some(dir) => Self::Dir(dir.to_path_buf()),
            None => Self::Listed(DAEMON.into_iter().map(Instance::named).collect()),
        }
    }

    /// Every daemon there could be, by instance name.
    ///
    /// An instance's directory outlives nothing: systemd removes it
    /// when the instance stops, so a socket that is there is one
    /// something should be answering on.  Whether anything does is the
    /// caller's to find out.
    pub fn instances(&self) -> Vec<Instance> {
        match self {
            Self::Listed(listed) => listed.clone(),
            Self::Dir(dir) => {
                let Ok(entries) = std::fs::read_dir(dir) else {
                    return Vec::new();
                };
                let mut sockets: Vec<Instance> = entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| Instance {
                        name: entry.file_name().to_string_lossy().into_owned(),
                        endpoint: entry.path().join("socket"),
                    })
                    .filter(|instance| instance.endpoint.exists())
                    .collect();
                sockets.sort();
                sockets
            }
        }
    }
}

/// How often the daemon polls each tier, from its [`Daemon::info`]
/// reply.
///
/// An older daemon does not report these, so each falls back to the
/// default rather than to zero.
pub fn cadence(info: &serde_json::Value) -> Cadence {
    let seconds = |name: &str, fallback: Duration| {
        info.get(name)
            .and_then(serde_json::Value::as_f64)
            .and_then(|s| Duration::try_from_secs_f64(s).ok())
            .filter(|d| !d.is_zero())
            .unwrap_or(fallback)
    };
    let default = Cadence::default();
    Cadence {
        fast: seconds("cadence_fast", default.fast),
        medium: seconds("cadence_medium", default.medium),
        slow: seconds("cadence_slow", default.slow),
    }
}

/// The receiver a daemon is attached to, from its [`Daemon::info`]
/// reply.
///
/// `None` when it is attached to nothing yet or its `*IDN?` did not
/// parse.
pub fn identity(info: &serde_json::Value) -> Option<Identity> {
    info.get("identity")
        .and_then(serde_json::Value::as_str)
        .and_then(|text| parse::identity(text).ok())
}

#[cfg(test)]
mod tests {
    use super::Daemon;
    use super::Daemons;
    use super::Instance;
    use super::cadence;
    use super::identity;
    use crate::link::listen_tcp;
    use crate::task::Cadence;
    use std::io::Write as _;
    use std::time::Duration;
    use std::time::Instant;

    #[test]
    fn sockets_lists_instances_with_a_socket_in_name_order() {
        let dir = std::env::temp_dir().join(format!("smartclock-daemons-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for name in ["b", "a", "stopped"] {
            std::fs::create_dir_all(dir.join(name)).expect("instance directory");
        }
        for name in ["b", "a"] {
            std::fs::write(dir.join(name).join("socket"), b"").expect("socket stand-in");
        }
        let found = Daemons::Dir(dir.clone()).instances();
        let _ = std::fs::remove_dir_all(&dir);
        let names: Vec<&str> = found.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(found[0].endpoint, dir.join("a").join("socket"));
    }

    #[test]
    fn sockets_of_a_missing_directory_is_empty() {
        let dir = std::env::temp_dir().join("smartclock-daemons-does-not-exist");
        assert!(Daemons::Dir(dir).instances().is_empty());
    }

    #[test]
    fn a_listed_daemon_is_named_or_goes_by_its_endpoint() {
        let named = Instance::named("bench=tcp://host:9000");
        assert_eq!(
            (named.name.as_str(), named.endpoint.as_path()),
            ("bench", std::path::Path::new("tcp://host:9000"))
        );
        let unnamed = Instance::named("/run/smartclockd/x/socket");
        assert_eq!(unnamed.name, "/run/smartclockd/x/socket");
        assert_eq!(
            unnamed.endpoint,
            std::path::Path::new("/run/smartclockd/x/socket")
        );
    }

    #[test]
    fn identity_comes_from_the_info_reply() {
        let info = serde_json::json!({ "identity": "HEWLETT-PACKARD,58503A,0000A00000,3704-C" });
        let id = identity(&info).expect("identity");
        assert_eq!(
            (id.model.as_str(), id.serial.as_str()),
            ("58503A", "0000A00000")
        );
        assert!(identity(&serde_json::json!({})).is_none());
        assert!(identity(&serde_json::json!({ "identity": "garbled" })).is_none());
    }

    #[test]
    fn cadence_comes_from_the_info_reply_and_falls_back_per_tier() {
        let info =
            serde_json::json!({ "cadence_fast": 2.0, "cadence_medium": 0.0, "cadence_slow": -1.0 });
        let got = cadence(&info);
        let default = Cadence::default();
        assert_eq!(got.fast, Duration::from_secs(2));
        assert_eq!(got.medium, default.medium, "zero is not a cadence");
        assert_eq!(got.slow, default.slow, "nor is a negative");
    }

    #[test]
    fn a_daemon_that_chatters_but_never_answers_is_given_up_on() {
        // Snapshots every tenth of a second and no reply.  Each snapshot
        // used to restart the read timeout, so the request never ended.
        let listener = listen_tcp("127.0.0.1:0").expect("listen");
        let path = listener.endpoint().expect("an endpoint");
        std::thread::spawn(move || {
            let Ok(mut stream) = listener.accept() else {
                return;
            };
            while writeln!(stream, r#"{{"snapshot":{{}}}}"#).is_ok() {
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        let started = Instant::now();
        let mut daemon =
            Daemon::connect_within(&path, Duration::from_millis(800)).expect("connect");
        let asked = daemon.info();
        assert!(asked.is_err());
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
    }
}
