//! The connection between a service and its clients: a Unix socket, or
//! TCP.
//!
//! A daemon binds a Unix socket at `--socket PATH`, and listens on TCP
//! at `--listen HOST:PORT`, either or both.  A client names the daemon
//! it asks, with `--daemon` or `--sensord`, by the socket's path or as
//! `tcp://HOST:PORT`.
//!
//! The socket is what a host's own clients use: file permissions decide
//! who may connect, and systemd's `RuntimeDirectory` decides how long it
//! is there.  Over TCP nothing decides who may connect.  Where there are
//! no Unix sockets, TCP is all there is.

use std::io;
use std::io::Read;
use std::io::Write;
use std::net::Shutdown;
use std::net::TcpListener;
use std::net::TcpStream;
use std::net::ToSocketAddrs as _;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use crate::client::DEADLINE;

#[cfg_attr(unix, path = "unix.rs")]
#[cfg_attr(windows, path = "windows.rs")]
mod os;

/// How a client names a TCP address.
const TCP_SCHEME: &str = "tcp://";

/// Where a client is told a service is.
#[derive(Debug, PartialEq, Eq)]
enum Endpoint<'a> {
    /// `tcp://HOST:PORT`.
    Tcp(&'a str),
    /// A Unix socket.
    Socket(&'a Path),
}

/// Read where `named` says a service is.
///
/// A bare `HOST:PORT` is refused rather than looked for as a socket of
/// that name, so the mistake is named rather than reported as a missing
/// file.
fn endpoint(named: &Path) -> io::Result<Endpoint<'_>> {
    let Some(text) = named.to_str() else {
        return Ok(Endpoint::Socket(named));
    };
    if let Some(address) = text.strip_prefix(TCP_SCHEME) {
        return Ok(Endpoint::Tcp(address));
    }
    if let Some((scheme, _)) = text.split_once("://") {
        return Err(invalid(format!("{text}: {scheme}:// is not {TCP_SCHEME}")));
    }
    if looks_like_an_address(text) {
        return Err(invalid(format!(
            "{text} would be a socket's path; a TCP address is {TCP_SCHEME}{text}"
        )));
    }
    Ok(Endpoint::Socket(named))
}

/// Whether `text` reads as `HOST:PORT` rather than as a path.
fn looks_like_an_address(text: &str) -> bool {
    !text.contains(['/', '\\'])
        && text
            .rsplit_once(':')
            .is_some_and(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok())
}

/// An error in how something was named.
fn invalid(why: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, why)
}

/// A connection to a service, from either end.
#[derive(Debug)]
pub struct Stream(Over);

/// What a [`Stream`] runs over.
#[derive(Debug)]
enum Over {
    /// A Unix socket.
    Unix(os::UnixStream),
    /// TCP.
    Tcp(TcpStream),
}

/// Apply the same call to whichever stream it is.
macro_rules! each {
    ($over:expr, $s:ident => $call:expr) => {
        match $over {
            Over::Unix($s) => $call,
            Over::Tcp($s) => $call,
        }
    };
}

impl From<TcpStream> for Stream {
    fn from(stream: TcpStream) -> Self {
        Self(Over::Tcp(stream))
    }
}

impl Stream {
    /// Another handle on the same connection, with the same timeouts.
    ///
    /// Carried over by hand: on Windows a handle's timeouts are its own,
    /// and a clone starts with none.
    pub fn try_clone(&self) -> io::Result<Self> {
        let clone = Self(match &self.0 {
            Over::Unix(s) => Over::Unix(s.try_clone()?),
            Over::Tcp(s) => Over::Tcp(s.try_clone()?),
        });
        clone.set_read_timeout(self.read_timeout()?)?;
        clone.set_write_timeout(self.write_timeout()?)?;
        Ok(clone)
    }

    /// How long a read may wait, if it is bounded.
    fn read_timeout(&self) -> io::Result<Option<Duration>> {
        each!(&self.0, s => s.read_timeout())
    }

    /// How long a write may wait, if it is bounded.
    fn write_timeout(&self) -> io::Result<Option<Duration>> {
        each!(&self.0, s => s.write_timeout())
    }

    /// Bound every read, or not when `None`.
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        each!(&self.0, s => s.set_read_timeout(timeout))
    }

    /// Bound every write, or not when `None`.
    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        each!(&self.0, s => s.set_write_timeout(timeout))
    }

    /// Shut the connection, for every handle on it.
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        each!(&self.0, s => s.shutdown(how))
    }

    /// Bound reads and writes by [`DEADLINE`], and send each line as
    /// it is written: requests and replies are a line each, and each is
    /// waited on.
    fn prepare(self) -> io::Result<Self> {
        if let Over::Tcp(s) = &self.0 {
            s.set_nodelay(true)?;
        }
        self.set_read_timeout(Some(DEADLINE))?;
        self.set_write_timeout(Some(DEADLINE))?;
        Ok(self)
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        each!(&mut self.0, s => s.read(buf))
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        each!(&mut self.0, s => s.write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        each!(&mut self.0, s => s.flush())
    }
}

/// Where a service accepts its clients.
#[derive(Debug)]
pub struct Listener(On);

/// What a [`Listener`] listens on.
#[derive(Debug)]
enum On {
    /// A Unix socket.
    Unix(os::UnixListener),
    /// A TCP port.
    Tcp(TcpListener),
}

impl Listener {
    /// Where a client reaches this listener, as [`connect`] takes it:
    /// the socket's path, or `tcp://` and the address bound, its port
    /// the one the system picked if asked for port 0.
    pub fn endpoint(&self) -> io::Result<PathBuf> {
        match &self.0 {
            On::Unix(l) => os::path(l),
            On::Tcp(l) => Ok(PathBuf::from(format!("{TCP_SCHEME}{}", l.local_addr()?))),
        }
    }

    /// The next client, with nothing set on its connection yet.
    pub fn accept(&self) -> io::Result<Stream> {
        Ok(Stream(match &self.0 {
            On::Unix(l) => Over::Unix(os::accept(l)?),
            On::Tcp(l) => Over::Tcp(l.accept()?.0),
        }))
    }
}

/// Connect to the service `named`, its socket or `tcp://HOST:PORT`,
/// with every read and write bounded by [`DEADLINE`].
///
/// A daemon that has accepted and then wedged would otherwise hold its
/// caller for good.
pub fn connect(named: &Path) -> io::Result<Stream> {
    let stream = match endpoint(named)? {
        Endpoint::Tcp(address) => Over::Tcp(connect_tcp(address)?),
        Endpoint::Socket(socket) => Over::Unix(os::connect(socket)?),
    };
    Stream(stream).prepare()
}

/// Connect to `HOST:PORT`, trying each address it resolves to.
fn connect_tcp(address: &str) -> io::Result<TcpStream> {
    let mut last = None;
    for resolved in address.to_socket_addrs()? {
        match TcpStream::connect_timeout(&resolved, DEADLINE) {
            Ok(stream) => return Ok(stream),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{address} resolves to no address"),
        )
    }))
}

/// Bind a Unix socket at `socket`, owner and group only, replacing one
/// left behind that nothing answers on.
pub fn listen(socket: &Path) -> io::Result<Listener> {
    if let Endpoint::Tcp(_) = endpoint(socket)? {
        return Err(invalid(
            "a socket is a path; listen on TCP with --listen HOST:PORT".to_owned(),
        ));
    }
    Ok(Listener(On::Unix(os::bind(socket)?)))
}

/// Listen where this platform's own clients would reach a service: on a
/// Unix socket of the process's own, named after `tag`, or where there
/// are none on a loopback port.  For tests, in this crate and others.
#[doc(hidden)]
pub fn listen_scratch(tag: &str) -> io::Result<(Listener, Scratch)> {
    let listener = match os::scratch(tag) {
        Some(socket) => listen(&socket)?,
        None => listen_tcp("127.0.0.1:0")?,
    };
    let endpoint = listener.endpoint()?;
    Ok((listener, Scratch(endpoint)))
}

/// Where a [`listen_scratch`] listener is; its socket, if it has one,
/// is removed when this goes.
#[doc(hidden)]
#[derive(Debug)]
pub struct Scratch(PathBuf);

impl Scratch {
    /// Where a client reaches the listener, as [`connect`] takes it.
    pub fn endpoint(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Ok(Endpoint::Socket(socket)) = endpoint(&self.0) {
            let _ = std::fs::remove_file(socket);
        }
    }
}

/// Listen on TCP at `address`, `HOST:PORT`.
///
/// Nothing decides who may connect: anyone who can reach the address
/// may issue whatever the service allows.
pub fn listen_tcp(address: &str) -> io::Result<Listener> {
    let address = listening_address(address).map_err(invalid)?;
    Ok(Listener(On::Tcp(TcpListener::bind(address)?)))
}

/// `text` if it is written as a listening address is, `HOST:PORT`.
///
/// For a command line to check when it is parsed: a daemon listens only
/// once its receiver has answered, which may be long after it started.
pub fn listening_address(text: &str) -> Result<String, String> {
    if text.contains("://") {
        return Err("a listening address is HOST:PORT, with no scheme".to_owned());
    }
    Ok(text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::Endpoint;
    use super::Listener;
    use super::connect;
    use super::endpoint;
    use super::listen;
    use super::listen_scratch;
    use super::listen_tcp;
    use std::io::BufRead as _;
    use std::io::BufReader;
    use std::io::Write as _;
    use std::path::Path;
    use std::time::Duration;

    /// Accept one client and echo one line back to it.
    fn echo_once(listener: Listener) {
        std::thread::spawn(move || {
            let Ok(stream) = listener.accept() else {
                return;
            };
            let mut line = String::new();
            let mut writer = stream.try_clone().expect("clone");
            if BufReader::new(stream).read_line(&mut line).is_ok() {
                let _ = writer.write_all(line.as_bytes());
            }
        });
    }

    fn round_trip(named: &Path) -> String {
        let mut stream = connect(named).expect("connect");
        stream.write_all(b"hello\n").expect("send");
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).expect("read");
        line
    }

    #[test]
    fn a_service_is_reached_where_it_listens() {
        let (listener, scratch) = listen_scratch("link").expect("listen");
        echo_once(listener);
        assert_eq!(round_trip(scratch.endpoint()), "hello\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_live_service_is_not_replaced() {
        let socket =
            std::env::temp_dir().join(format!("smartclock-link-live-{}", std::process::id()));
        let _listener = listen(&socket).expect("listen");
        let refused = listen(&socket).expect_err("a second listener");
        let _ = std::fs::remove_file(&socket);
        assert_eq!(refused.kind(), std::io::ErrorKind::AddrInUse);
    }

    #[test]
    fn a_service_is_reached_over_tcp() {
        let listener = listen_tcp("127.0.0.1:0").expect("listen");
        let named = listener.endpoint().expect("an endpoint");
        echo_once(listener);
        assert_eq!(round_trip(&named), "hello\n");
    }

    #[test]
    fn a_clone_keeps_the_timeouts() {
        let (listener, scratch) = listen_scratch("link").expect("listen");
        echo_once(listener);
        let stream = connect(scratch.endpoint()).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("read timeout");
        let clone = stream.try_clone().expect("clone");
        assert_eq!(
            clone.read_timeout().expect("read timeout"),
            Some(Duration::from_secs(3))
        );
        assert_eq!(
            clone.write_timeout().expect("write timeout"),
            Some(crate::client::DEADLINE)
        );
    }

    #[test]
    fn a_client_names_a_socket_or_a_tcp_address() {
        assert_eq!(
            endpoint(Path::new("tcp://host:9000")).expect("tcp"),
            Endpoint::Tcp("host:9000")
        );
        assert_eq!(
            endpoint(Path::new("/run/x/socket")).expect("a path"),
            Endpoint::Socket(Path::new("/run/x/socket"))
        );
        for refused in [
            "host:9000",
            "0.0.0.0:9000",
            "unix:///run/x",
            "tls://host:9000",
        ] {
            assert_eq!(
                endpoint(Path::new(refused)).expect_err(refused).kind(),
                std::io::ErrorKind::InvalidInput,
                "{refused}"
            );
        }
    }

    #[test]
    fn a_socket_is_a_path_and_a_listening_address_has_no_scheme() {
        let refused = listen(Path::new("tcp://127.0.0.1:0")).expect_err("a TCP socket");
        assert_eq!(refused.kind(), std::io::ErrorKind::InvalidInput);
        let refused = listen_tcp("tcp://127.0.0.1:0").expect_err("a scheme");
        assert_eq!(refused.kind(), std::io::ErrorKind::InvalidInput);
    }
}
