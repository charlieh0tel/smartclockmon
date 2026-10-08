//! The connection between a service and its clients: a Unix socket, or
//! TCP.
//!
//! `--socket` and `--listen` name where a service is the same way, at
//! either end: `tcp://HOST:PORT`, or a path, bare or as `unix://PATH`.
//! A path is a Unix socket, or a file naming the TCP address the
//! service listens on.  The file stands where the socket would, in the
//! service's run directory, so either way systemd's `RuntimeDirectory`
//! decides how long it is there.
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

/// How a path may be written, for symmetry with [`TCP_SCHEME`].
const UNIX_SCHEME: &str = "unix://";

/// Where a service is, as `--socket` and `--listen` name it.
#[derive(Debug, PartialEq, Eq)]
enum Endpoint<'a> {
    /// `tcp://HOST:PORT`.
    Tcp(&'a str),
    /// A Unix socket, or a file naming a TCP address.
    Path(&'a Path),
}

/// Read where `named` says a service is.
///
/// A bare `HOST:PORT` is refused rather than taken as a file of that
/// name, which listening would create in the working directory.
fn endpoint(named: &Path) -> io::Result<Endpoint<'_>> {
    let Some(text) = named.to_str() else {
        return Ok(Endpoint::Path(named));
    };
    if let Some(address) = text.strip_prefix(TCP_SCHEME) {
        return Ok(Endpoint::Tcp(address));
    }
    if let Some(path) = text.strip_prefix(UNIX_SCHEME) {
        return Ok(Endpoint::Path(Path::new(path)));
    }
    let refuse = |why: String| Err(io::Error::new(io::ErrorKind::InvalidInput, why));
    if let Some((scheme, _)) = text.split_once("://") {
        return refuse(format!(
            "{text}: {scheme}:// is not {TCP_SCHEME} or {UNIX_SCHEME}"
        ));
    }
    if looks_like_an_address(text) {
        return refuse(format!(
            "{text} would be a file name; a TCP address is {TCP_SCHEME}{text}"
        ));
    }
    Ok(Endpoint::Path(named))
}

/// Whether `text` reads as `HOST:PORT` rather than as a path.
fn looks_like_an_address(text: &str) -> bool {
    !text.contains(['/', '\\'])
        && text
            .rsplit_once(':')
            .is_some_and(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok())
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
    let stream = match endpoint(socket)? {
        Endpoint::Tcp(address) => Stream::Tcp(connect_tcp(address)?),
        Endpoint::Path(path) => connect_file(path)?,
    };
    stream.prepare()
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

/// Listen where `named` says: on TCP at a `tcp://` address, and at a
/// path on a Unix socket, owner and group only, where there are any,
/// and elsewhere on a loopback port named in the file.
///
/// Over TCP nothing decides who may connect: anyone who can reach the
/// address may issue whatever the service allows.
pub fn listen(named: &Path) -> io::Result<Listener> {
    match endpoint(named)? {
        Endpoint::Tcp(address) => Ok(Listener::Tcp(TcpListener::bind(address)?)),
        Endpoint::Path(path) => listen_at(path),
    }
}

/// Listen at a path.
///
/// A socket left behind by a crash would otherwise block the bind, so
/// it is replaced -- but only if nothing answers on it.  Replacing a
/// live service's left that service running and unreachable, and its
/// clients reconnecting to this one.
fn listen_at(socket: &Path) -> io::Result<Listener> {
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if connect_file(socket).is_ok() {
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
    use super::Endpoint;
    use super::Stream;
    use super::connect;
    use super::endpoint;
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
    fn an_endpoint_is_a_tcp_address_or_a_path() {
        assert_eq!(
            endpoint(Path::new("tcp://host:9000")).expect("tcp"),
            Endpoint::Tcp("host:9000")
        );
        assert_eq!(
            endpoint(Path::new("unix:///run/x/socket")).expect("unix"),
            Endpoint::Path(Path::new("/run/x/socket"))
        );
        assert_eq!(
            endpoint(Path::new("/run/x/socket")).expect("a path"),
            Endpoint::Path(Path::new("/run/x/socket"))
        );
        for refused in ["host:9000", "0.0.0.0:9000", "tls://host:9000"] {
            assert_eq!(
                endpoint(Path::new(refused)).expect_err(refused).kind(),
                std::io::ErrorKind::InvalidInput,
                "{refused}"
            );
        }
    }

    #[test]
    fn a_service_listens_on_a_tcp_address_it_is_given() {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
        let named = format!("tcp://{}", probe.local_addr().expect("an address"));
        drop(probe);
        echo_once(listen(Path::new(&named)).expect("listen"));
        assert_eq!(round_trip(Path::new(&named)), "hello\n");
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
