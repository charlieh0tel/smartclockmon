//! Serving the client protocol on a local socket.
//!
//! In the library because more than one service serves it: the
//! receiver daemon and the sensor service.  Each says what it answers
//! through [`Service`]; what is here is what every one of them needs
//! and must not get wrong separately -- a bounded number of clients, a
//! bounded request line, a write deadline, and a socket only its owner
//! and group can open.
//!
//! Authorization is socket permissions and nothing else: systemd's
//! `RuntimeDirectory` and its mode decide who can open the socket, and
//! anyone who can may issue whatever the service allows.

use std::io::BufRead as _;
use std::io::BufReader;
use std::io::Read as _;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use interprocess::local_socket::GenericFilePath;
use interprocess::local_socket::Listener;
use interprocess::local_socket::ListenerOptions;
use interprocess::local_socket::SendHalf;
use interprocess::local_socket::Stream;
use interprocess::local_socket::ToFsName as _;
use interprocess::local_socket::traits::ListenerExt as _;
use interprocess::local_socket::traits::Stream as _;

use crate::client;
use crate::protocol::Message;
use crate::protocol::Protocol;
use crate::protocol::Request;

/// Longest request line accepted.
///
/// A line is read until a newline, so without a cap a client that
/// never sends one grows a single string in the service for as long as
/// it keeps writing.  A status screen query is a few dozen bytes; this
/// is generous.
pub const MAX_REQUEST: u64 = 8192;

/// How many clients may be connected at once.
///
/// Each takes a thread or two.  Socket permissions decide who may
/// connect at all, but nothing stopped one mistaken loop from opening
/// connections until the service ran out of threads, and the failure
/// mode for that used to be a service that looked healthy and could
/// never be reached again.
pub const MAX_CLIENTS: usize = 16;

/// How long the accept loop waits after a failed accept.  Long enough
/// that a persistent failure costs nothing, short next to a client's
/// patience.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(200);

/// The half of a client's connection the service writes to, shared by
/// whatever threads write to it.
pub type Writer = Arc<Mutex<SendHalf>>;

/// What a service answers, and anything it sends unasked.
pub trait Service: Send + Sync + 'static {
    /// The requests it answers.
    type Op: Protocol;

    /// Answer one request, already checked to be of this protocol's
    /// version.
    fn answer(&self, id: String, op: Self::Op) -> Message;

    /// Called once a client has connected, before its first request.
    ///
    /// A service that sends messages unasked starts doing so here.  The
    /// thread that does holds the client's [`Held`], so the client
    /// counts against [`MAX_CLIENTS`] for as long as anything is being
    /// sent to it, and is let go when that thread ends; it stops once
    /// `serving` is false.  A service that sends nothing unasked gives
    /// `held` back, and the client is let go when it stops asking.
    fn connected(
        &self,
        writer: &Writer,
        serving: &Arc<AtomicBool>,
        held: Held,
    ) -> std::io::Result<Option<Held>> {
        let _ = (writer, serving);
        Ok(Some(held))
    }
}

/// What a connected client holds: its place among [`MAX_CLIENTS`], and
/// its socket's write deadline and shutdown.  Dropping it lets the
/// client go.
#[derive(Debug)]
pub struct Held {
    _slot: Slot,
    _closer: Closer,
}

/// Bind `socket`, owner and group only.
///
/// A socket left behind by a crash would otherwise block the bind, so
/// it is removed -- but only if nothing answers on it.  Removing a live
/// service's socket left that service running and unreachable, and its
/// clients reconnecting to this one.  Bound here rather than in the
/// serving thread, so a failure reaches whoever started the service.
pub fn listen(socket: &Path) -> std::io::Result<Listener> {
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = socket.to_fs_name::<GenericFilePath>()?.into_owned();
    if Stream::connect(name.clone()).is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            format!(
                "another service is serving {}; stop it first",
                socket.display()
            ),
        ));
    }
    let _ = std::fs::remove_file(socket);
    let listener = ListenerOptions::new().name(name).create_sync()?;
    // Socket permissions are the whole of the authorization model, so
    // they are set rather than inherited from whatever umask the
    // service happened to start with.  Group access is deliberate: it
    // is how an unprivileged operator runs the monitor.
    set_socket_mode(socket)?;
    Ok(listener)
}

/// Give the socket owner and group access, and nobody else.
#[cfg(unix)]
fn set_socket_mode(socket: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o660))
}

/// Elsewhere the platform decides; there is no portable equivalent.
#[cfg(not(unix))]
fn set_socket_mode(_socket: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Listen for clients until the process ends.  `program` names the
/// service in its threads and its messages.
pub fn serve<S: Service>(listener: Listener, program: &'static str, service: Arc<S>) {
    let clients = Arc::new(AtomicUsize::new(0));
    let mut failing = false;
    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(stream) => {
                failing = false;
                stream
            }
            // One client failing to connect is not a reason to stop
            // serving the others.  But an error that persists -- out of
            // file descriptors, say -- comes straight back, so the loop
            // pauses rather than spinning, and says so once.
            Err(e) => {
                if !failing {
                    eprintln!("{program}: rejected a connection: {e}");
                    failing = true;
                }
                thread::sleep(ACCEPT_BACKOFF);
                continue;
            }
        };
        if clients.load(Ordering::Relaxed) >= MAX_CLIENTS {
            eprintln!("{program}: refusing a client, {MAX_CLIENTS} already connected");
            // Say so rather than closing silently: a bare reset reads
            // as the service having crashed, which is the wrong thing
            // for an operator to go and investigate.
            let writer = Arc::new(Mutex::new(stream));
            let _ = write_line(
                &writer,
                &Message::err_in(
                    S::Op::VERSION,
                    String::new(),
                    format!("{MAX_CLIENTS} clients are already connected"),
                ),
            );
            continue;
        }
        let service = Arc::clone(&service);
        let slot = Slot::take(&clients);
        // A spawn failure must not end the accept loop.  It used to
        // propagate, so one transient EAGAIN under thread pressure left
        // the service looking healthy while no client could ever
        // connect again.
        let spawned = thread::Builder::new()
            .name(format!("{program}-client"))
            .spawn(move || {
                // The slot is released when whatever holds it goes,
                // whether it returns or unwinds.  Decrementing on the
                // way out by hand leaked a slot permanently on a panic,
                // sixteen of which would have left the service accepting
                // nobody.
                if let Err(e) = talk(stream, service.as_ref(), slot) {
                    eprintln!("{program}: client ended: {e}");
                }
            });
        if let Err(e) = spawned {
            eprintln!("{program}: could not serve a client: {e}");
        }
    }
}

/// Serve one client until it goes away.
fn talk<S: Service>(stream: Stream, service: &S, slot: Slot) -> std::io::Result<()> {
    let held = Held {
        _closer: Closer::for_stream(&stream),
        _slot: slot,
    };
    let (recv, send_half) = stream.split();
    let writer: Writer = Arc::new(Mutex::new(send_half));

    // Tells anything sending unasked to stop when this thread does,
    // however it ends.
    let serving = Arc::new(AtomicBool::new(true));
    let _hangup = Hangup(Arc::clone(&serving));
    // Kept here, if the service gave it back, until the client stops
    // asking.
    let _held = service.connected(&writer, &serving, held)?;

    // Capped: an unterminated line would otherwise grow without limit.
    let mut reader = BufReader::new(recv);
    let version = S::Op::VERSION;
    loop {
        let mut line = String::new();
        let read = (&mut reader).take(MAX_REQUEST + 1).read_line(&mut line)?;
        if read == 0 {
            return Ok(());
        }
        if read as u64 > MAX_REQUEST {
            write_line(
                &writer,
                &Message::err_in(
                    version,
                    String::new(),
                    format!("a request may not exceed {MAX_REQUEST} bytes"),
                ),
            )?;
            return Ok(());
        }
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Request<S::Op>>(&line) {
            Ok(request) if request.v != version => Message::err_in(
                version,
                request.id,
                format!("this service speaks protocol {version}, not {}", request.v),
            ),
            Ok(request) => service.answer(request.id, request.op),
            Err(e) => Message::err_in(version, String::new(), format!("malformed request: {e}")),
        };
        write_line(&writer, &reply)?;
    }
}

/// Write one message as a line.
pub fn write_line<W: Write>(writer: &Mutex<W>, message: &Message) -> std::io::Result<()> {
    let line = serde_json::to_string(message)?;
    let mut writer = writer
        .lock()
        .map_err(|_| std::io::Error::other("poisoned writer"))?;
    writer.write_all(line.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()
}

/// Clears a flag when it goes, whatever ended the thread holding it.
struct Hangup(Arc<AtomicBool>);

impl Drop for Hangup {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

/// A client's socket, held with its slot: it bounds every write to
/// [`client::DEADLINE`], and shuts the socket both ways when dropped.
///
/// Without the deadline a client that stopped reading filled its buffer
/// and left a writer blocked for good, holding the slot and its
/// threads.  Without the shutdown a thread sending unasked that ended
/// left the request thread reading a socket nobody would close.  Unix
/// only: `interprocess`'s portable stream has neither, and elsewhere
/// this does nothing.
#[derive(Debug)]
struct Closer {
    #[cfg(unix)]
    socket: Option<socket2::Socket>,
}

impl Closer {
    #[cfg(unix)]
    fn new(fd: std::os::fd::BorrowedFd<'_>) -> Self {
        let socket = fd.try_clone_to_owned().ok().map(socket2::Socket::from);
        if let Some(socket) = &socket {
            let _ = socket.set_write_timeout(Some(client::DEADLINE));
        }
        Self { socket }
    }

    #[cfg(not(unix))]
    fn new() -> Self {
        Self {}
    }

    /// For a stream as the listener hands it over.
    fn for_stream(stream: &Stream) -> Self {
        #[cfg(unix)]
        {
            use std::os::fd::AsFd as _;
            let Stream::UdSocket(stream) = stream;
            Self::new(stream.as_fd())
        }
        #[cfg(not(unix))]
        {
            let _ = stream;
            Self::new()
        }
    }
}

impl Drop for Closer {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(socket) = &self.socket {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// One connected client, counted for as long as this is held.
///
/// The count is what [`MAX_CLIENTS`] is enforced against, so releasing
/// it has to survive the serving thread panicking.
#[derive(Debug)]
struct Slot(Arc<AtomicUsize>);

impl Slot {
    fn take(clients: &Arc<AtomicUsize>) -> Self {
        clients.fetch_add(1, Ordering::Relaxed);
        Self(Arc::clone(clients))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::Closer;
    use super::MAX_CLIENTS;
    use super::MAX_REQUEST;
    use super::Service;
    use super::listen;
    use super::serve;
    use crate::protocol::Message;
    use crate::protocol::Protocol;

    use serde::Deserialize;
    use serde::Serialize;
    use std::io::BufRead as _;
    use std::io::BufReader;
    use std::io::Read as _;
    use std::io::Write as _;
    use std::os::fd::AsFd as _;
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    /// A service that answers one request and sends nothing unasked.
    struct Echo;

    #[derive(Debug, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "lowercase")]
    enum Ping {
        Ping,
    }

    impl Protocol for Ping {
        const VERSION: u32 = 3;
    }

    impl Service for Echo {
        type Op = Ping;
        fn answer(&self, id: String, _op: Ping) -> Message {
            Message::ok_in(Ping::VERSION, id, serde_json::json!("pong"))
        }
    }

    /// A service on a socket of its own, removed when this goes.
    struct Running(PathBuf);

    impl Running {
        fn start(name: &str) -> Self {
            let socket = std::env::temp_dir().join(format!(
                "smartclock-server-test-{name}-{}",
                std::process::id()
            ));
            let listener = listen(&socket).expect("listen");
            thread::spawn(move || serve(listener, "test", Arc::new(Echo)));
            Self(socket)
        }

        fn connect(&self) -> UnixStream {
            let stream = UnixStream::connect(&self.0).expect("connect");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("timeout");
            stream
        }
    }

    impl Drop for Running {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn first_line(stream: &UnixStream) -> String {
        let mut line = String::new();
        BufReader::new(stream.try_clone().expect("clone"))
            .read_line(&mut line)
            .expect("read");
        line
    }

    #[test]
    fn a_request_is_answered_in_the_services_version() {
        let running = Running::start("answer");
        let mut client = running.connect();
        writeln!(client, r#"{{"v":3,"id":"a","op":{{"kind":"ping"}}}}"#).expect("send");
        let line = first_line(&client);
        assert!(line.contains(r#""v":3"#) && line.contains("pong"), "{line}");
    }

    #[test]
    fn a_request_in_another_version_is_refused() {
        let running = Running::start("version");
        let mut client = running.connect();
        writeln!(client, r#"{{"v":1,"id":"a","op":{{"kind":"ping"}}}}"#).expect("send");
        let line = first_line(&client);
        assert!(line.contains("speaks protocol 3, not 1"), "{line}");
    }

    #[test]
    fn the_socket_is_not_world_accessible() {
        use std::os::unix::fs::PermissionsExt as _;
        let running = Running::start("mode");
        let mode = std::fs::metadata(&running.0)
            .expect("stat the socket")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o660, "the socket is {mode:o}, not 0660");
    }

    #[test]
    fn a_live_socket_is_not_taken_over() {
        let running = Running::start("live");
        let refused = listen(&running.0).expect_err("a second listener");
        assert_eq!(refused.kind(), std::io::ErrorKind::AddrInUse);
    }

    #[test]
    fn a_request_longer_than_the_cap_is_refused() {
        let running = Running::start("cap");
        let mut client = running.connect();
        // Unterminated: only the capped read catches a line that never
        // ends.
        write!(client, "{}", "x".repeat(MAX_REQUEST as usize + 100)).expect("write");
        assert!(first_line(&client).contains("may not exceed"));
    }

    #[test]
    fn a_client_past_the_cap_is_turned_away_and_a_departed_one_makes_room() {
        let running = Running::start("clients");
        let held: Vec<_> = (0..MAX_CLIENTS).map(|_| running.connect()).collect();
        let extra = running.connect();
        assert!(first_line(&extra).contains("already connected"));
        // A service that sends nothing unasked lets a client go when it
        // stops asking.
        drop(held);
        thread::sleep(Duration::from_millis(300));
        let mut client = running.connect();
        writeln!(client, r#"{{"v":3,"id":"b","op":{{"kind":"ping"}}}}"#).expect("send");
        assert!(first_line(&client).contains("pong"));
    }

    #[test]
    fn a_client_socket_has_a_write_deadline_and_is_shut_when_its_closer_goes() {
        let (service_side, mut client_side) = UnixStream::pair().expect("a socket pair");
        let closer = Closer::new(service_side.as_fd());
        assert_eq!(
            service_side.write_timeout().expect("the write timeout"),
            Some(crate::client::DEADLINE)
        );
        client_side
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        drop(closer);
        // End of stream, not a timeout: the socket was shut, though the
        // service side's own descriptor is still open.
        let mut byte = [0u8; 1];
        assert_eq!(client_side.read(&mut byte).expect("end of stream"), 0);
    }
}
