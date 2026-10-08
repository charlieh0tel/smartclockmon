//! The connection between a service and its clients: a Unix socket, or
//! TCP.
//!
//! A service is found by a path, as `--socket` names it: a Unix socket,
//! or a file naming the TCP address it listens on.  The file stands
//! where the socket would, in the service's run directory, so either
//! way systemd's `RuntimeDirectory` decides how long it is there.  A
//! client may also name a TCP address outright, as `tcp://HOST:PORT`.
//!
//! Unix sockets are the default where there are any: file permissions
//! decide who may connect.  Elsewhere a service listens on a loopback
//! port the system picks, and anything on the host may connect.

use std::io;
use std::io::Read;
use std::io::Write;
#[cfg(any(not(unix), test))]
use std::net::Ipv4Addr;
use std::net::Shutdown;
#[cfg(any(not(unix), test))]
use std::net::SocketAddr;
use std::net::TcpListener;
use std::net::TcpStream;
use std::net::ToSocketAddrs as _;
#[cfg(unix)]
use std::os::unix::fs::FileTypeExt as _;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
#[cfg(unix)]
use std::os::unix::net::UnixListener;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use crate::client::DEADLINE;

/// How a TCP address is written, in a socket file or in place of one.
const TCP_SCHEME: &str = "tcp://";

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
    /// The next client, with nothing set on its connection yet.
    pub fn accept(&self) -> io::Result<Stream> {
        Ok(match self {
            #[cfg(unix)]
            Self::Unix(l) => Stream::Unix(l.accept()?.0),
            Self::Tcp(l) => Stream::Tcp(l.accept()?.0),
        })
    }
}

/// Connect to the service at `socket`, with every read and write
/// bounded by [`DEADLINE`].
///
/// A daemon that has accepted and then wedged would otherwise hold its
/// caller for good.
pub fn connect(socket: &Path) -> io::Result<Stream> {
    let stream = match tcp_named(socket) {
        Some(address) => Stream::Tcp(connect_tcp(address)?),
        None => connect_file(socket)?,
    };
    stream.prepare()
}

/// The address `socket` names outright, if it is one.
fn tcp_named(socket: &Path) -> Option<&str> {
    socket.to_str()?.strip_prefix(TCP_SCHEME)
}

/// Connect to the service whose socket, or whose address, is in a file.
fn connect_file(socket: &Path) -> io::Result<Stream> {
    #[cfg(unix)]
    if std::fs::metadata(socket)?.file_type().is_socket() {
        return Ok(Stream::Unix(UnixStream::connect(socket)?));
    }
    let text = std::fs::read_to_string(socket)?;
    let address = text.trim().strip_prefix(TCP_SCHEME).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} is neither a socket nor a {TCP_SCHEME} address",
                socket.display()
            ),
        )
    })?;
    Ok(Stream::Tcp(connect_tcp(address)?))
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

/// Listen at `socket`: a Unix socket, owner and group only, where there
/// are any, and elsewhere a loopback port named in the file.
///
/// A socket left behind by a crash would otherwise block the bind, so
/// it is replaced -- but only if nothing answers on it.  Replacing a
/// live service's left that service running and unreachable, and its
/// clients reconnecting to this one.
pub fn listen(socket: &Path) -> io::Result<Listener> {
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if connect(socket).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!(
                "another service is serving {}; stop it first",
                socket.display()
            ),
        ));
    }
    #[cfg(unix)]
    {
        let _ = std::fs::remove_file(socket);
        let listener = UnixListener::bind(socket)?;
        // Socket permissions are the whole of the authorization model
        // here, so they are set rather than inherited from whatever
        // umask the service happened to start with.  Group access is
        // deliberate: it is how an unprivileged operator runs the
        // monitor.
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o660))?;
        Ok(Listener::Unix(listener))
    }
    #[cfg(not(unix))]
    listen_loopback(socket)
}

/// Listen on a loopback port the system picks, and name it in `socket`.
#[cfg(any(not(unix), test))]
fn listen_loopback(socket: &Path) -> io::Result<Listener> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    publish(socket, listener.local_addr()?)?;
    Ok(Listener::Tcp(listener))
}

/// Name `address` in `file`, replacing it whole: a client reading while
/// it is written sees the old address or the new one, never part of
/// either.
#[cfg(any(not(unix), test))]
fn publish(file: &Path, address: SocketAddr) -> io::Result<()> {
    let mut staged = file.as_os_str().to_owned();
    staged.push(".new");
    std::fs::write(&staged, format!("{TCP_SCHEME}{address}\n"))?;
    std::fs::rename(&staged, file)
}

#[cfg(test)]
mod tests {
    use super::Stream;
    use super::connect;
    use super::listen;
    use super::listen_loopback;
    use std::io::BufRead as _;
    use std::io::BufReader;
    use std::io::Write as _;
    use std::path::Path;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("smartclock-link-{name}-{}", std::process::id()))
    }

    /// Accept one client and echo one line back to it.
    fn echo_once(listener: super::Listener) {
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

    fn round_trip(socket: &Path) -> String {
        let mut stream = connect(socket).expect("connect");
        stream.write_all(b"hello\n").expect("send");
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).expect("read");
        line
    }

    #[test]
    fn a_service_is_reached_through_its_socket() {
        let socket = scratch("socket");
        echo_once(listen(&socket).expect("listen"));
        let line = round_trip(&socket);
        let _ = std::fs::remove_file(&socket);
        assert_eq!(line, "hello\n");
    }

    #[test]
    fn a_service_is_reached_through_the_address_in_its_file() {
        let socket = scratch("file");
        echo_once(listen_loopback(&socket).expect("listen"));
        let line = round_trip(&socket);
        let _ = std::fs::remove_file(&socket);
        assert_eq!(line, "hello\n");
    }

    #[test]
    fn a_service_is_reached_by_address_outright() {
        let socket = scratch("outright");
        echo_once(listen_loopback(&socket).expect("listen"));
        let named = std::fs::read_to_string(&socket).expect("the address");
        let _ = std::fs::remove_file(&socket);
        assert_eq!(round_trip(Path::new(named.trim())), "hello\n");
    }

    #[test]
    fn a_live_service_is_not_replaced() {
        let socket = scratch("live");
        let _listener = listen_loopback(&socket).expect("listen");
        let refused = listen(&socket).expect_err("a second listener");
        let _ = std::fs::remove_file(&socket);
        assert_eq!(refused.kind(), std::io::ErrorKind::AddrInUse);
    }

    #[test]
    fn a_file_naming_no_address_says_so() {
        let socket = scratch("garbled");
        std::fs::write(&socket, b"nonsense\n").expect("write");
        let got = connect(&socket).map(|_: Stream| ());
        let _ = std::fs::remove_file(&socket);
        assert_eq!(
            got.expect_err("no address").kind(),
            std::io::ErrorKind::InvalidData
        );
    }
}
