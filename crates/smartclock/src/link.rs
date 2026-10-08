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
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
#[cfg(unix)]
use std::os::unix::net::UnixListener;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use crate::client::DEADLINE;

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
pub enum Stream {
    /// Over a Unix socket.
    #[cfg(unix)]
    Unix(UnixStream),
    /// Over TCP.
    Tcp(TcpStream),
}

/// Apply the same call to whichever stream it is.
macro_rules! each {
    ($stream:expr, $s:ident => $call:expr) => {
        match $stream {
            #[cfg(unix)]
            Stream::Unix($s) => $call,
            Stream::Tcp($s) => $call,
        }
    };
}

impl Stream {
    /// Another handle on the same connection.
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(match self {
            #[cfg(unix)]
            Self::Unix(s) => Self::Unix(s.try_clone()?),
            Self::Tcp(s) => Self::Tcp(s.try_clone()?),
        })
    }

    /// Bound every read, or not when `None`.
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        each!(self, s => s.set_read_timeout(timeout))
    }

    /// Bound every write, or not when `None`.
    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        each!(self, s => s.set_write_timeout(timeout))
    }

    /// Shut the connection, for every handle on it.
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        each!(self, s => s.shutdown(how))
    }

    /// Bound reads and writes by [`DEADLINE`], and send each line as
    /// it is written: requests and replies are a line each, and each is
    /// waited on.
    fn prepare(self) -> io::Result<Self> {
        match &self {
            #[cfg(unix)]
            Self::Unix(_) => {}
            Self::Tcp(s) => s.set_nodelay(true)?,
        }
        self.set_read_timeout(Some(DEADLINE))?;
        self.set_write_timeout(Some(DEADLINE))?;
        Ok(self)
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        each!(self, s => s.read(buf))
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        each!(self, s => s.write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        each!(self, s => s.flush())
    }
}

/// Where a service accepts its clients.
#[derive(Debug)]
pub enum Listener {
    /// On a Unix socket.
    #[cfg(unix)]
    Unix(UnixListener),
    /// On a TCP port.
    Tcp(TcpListener),
}

impl Listener {
    /// Where a client reaches this listener, as [`connect`] takes it:
    /// the socket's path, or `tcp://` and the address bound, its port
    /// the one the system picked if asked for port 0.
    pub fn endpoint(&self) -> io::Result<PathBuf> {
        match self {
            #[cfg(unix)]
            Self::Unix(l) => l
                .local_addr()?
                .as_pathname()
                .map(Path::to_path_buf)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::AddrNotAvailable, "an unnamed socket")
                }),
            Self::Tcp(l) => Ok(PathBuf::from(format!("{TCP_SCHEME}{}", l.local_addr()?))),
        }
    }

    /// The next client, with nothing set on its connection yet.
    pub fn accept(&self) -> io::Result<Stream> {
        Ok(match self {
            #[cfg(unix)]
            Self::Unix(l) => Stream::Unix(l.accept()?.0),
            Self::Tcp(l) => Stream::Tcp(l.accept()?.0),
        })
    }
}

/// Connect to the service `named`, its socket or `tcp://HOST:PORT`,
/// with every read and write bounded by [`DEADLINE`].
///
/// A daemon that has accepted and then wedged would otherwise hold its
/// caller for good.
pub fn connect(named: &Path) -> io::Result<Stream> {
    let stream = match endpoint(named)? {
        Endpoint::Tcp(address) => Stream::Tcp(connect_tcp(address)?),
        Endpoint::Socket(socket) => connect_socket(socket)?,
    };
    stream.prepare()
}

/// Connect to a Unix socket.
#[cfg(unix)]
fn connect_socket(socket: &Path) -> io::Result<Stream> {
    Ok(Stream::Unix(UnixStream::connect(socket)?))
}

/// There are no Unix sockets here.
#[cfg(not(unix))]
fn connect_socket(socket: &Path) -> io::Result<Stream> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "{}: there are no Unix sockets here; name the service as {TCP_SCHEME}HOST:PORT",
            socket.display()
        ),
    ))
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

/// Bind a Unix socket at `socket`, owner and group only.
///
/// A socket left behind by a crash would otherwise block the bind, so
/// it is replaced -- but only if nothing answers on it.  Replacing a
/// live service's left that service running and unreachable, and its
/// clients reconnecting to this one.
pub fn listen(socket: &Path) -> io::Result<Listener> {
    if let Endpoint::Tcp(_) = endpoint(socket)? {
        return Err(invalid(
            "a socket is a path; listen on TCP with --listen HOST:PORT".to_owned(),
        ));
    }
    bind_socket(socket)
}

#[cfg(unix)]
fn bind_socket(socket: &Path) -> io::Result<Listener> {
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if connect_socket(socket).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!(
                "another service is serving {}; stop it first",
                socket.display()
            ),
        ));
    }
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)?;
    // Socket permissions are the whole of the authorization model, so
    // they are set rather than inherited from whatever umask the
    // service happened to start with.  Group access is deliberate: it
    // is how an unprivileged operator runs the monitor.
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o660))?;
    Ok(Listener::Unix(listener))
}

#[cfg(not(unix))]
fn bind_socket(socket: &Path) -> io::Result<Listener> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "{}: there are no Unix sockets here; listen on TCP with --listen HOST:PORT",
            socket.display()
        ),
    ))
}

/// Listen on TCP at `address`, `HOST:PORT`.
///
/// Nothing decides who may connect: anyone who can reach the address
/// may issue whatever the service allows.
pub fn listen_tcp(address: &str) -> io::Result<Listener> {
    let address = listening_address(address).map_err(invalid)?;
    Ok(Listener::Tcp(TcpListener::bind(address)?))
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
    use super::listen_tcp;
    use std::io::BufRead as _;
    use std::io::BufReader;
    use std::io::Write as _;
    use std::path::Path;

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

    #[cfg(unix)]
    #[test]
    fn a_service_is_reached_through_its_socket() {
        let socket =
            std::env::temp_dir().join(format!("smartclock-link-socket-{}", std::process::id()));
        echo_once(listen(&socket).expect("listen"));
        let line = round_trip(&socket);
        let _ = std::fs::remove_file(&socket);
        assert_eq!(line, "hello\n");
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
