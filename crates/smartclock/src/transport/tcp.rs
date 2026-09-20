//! A receiver reached over TCP.
//!
//! For a serial adapter on the network, such as one behind ser2net, and
//! for the simulator, which listens on TCP so the daemon can attach to
//! it on any platform.  A pseudo-terminal would have been the obvious
//! way to fake a serial device and would have confined the simulator to
//! Unix.

use std::io::Read;
use std::io::Write;
use std::net::TcpStream;
use std::net::ToSocketAddrs;
use std::time::Duration;

use crate::error::Result;
use crate::transport::Transport;

/// A receiver on the other end of a TCP connection.
#[derive(Debug)]
pub struct TcpTransport {
    stream: TcpStream,
    address: String,
}

impl TcpTransport {
    /// Connect, and set the read timeout the session layer expects.
    pub fn connect(address: &str, read_timeout: Duration) -> Result<Self> {
        let resolved = address
            .to_socket_addrs()
            .map_err(crate::error::Error::Io)?
            .next()
            .ok_or_else(|| {
                crate::error::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::AddrNotAvailable,
                    format!("{address} resolved to nothing"),
                ))
            })?;
        let stream = TcpStream::connect(resolved)?;
        stream.set_read_timeout(Some(read_timeout))?;
        // Replies are short and latency matters more than packing, so
        // Nagle only delays the prompt the session is waiting for.
        stream.set_nodelay(true)?;
        Ok(Self {
            stream,
            address: address.to_owned(),
        })
    }
}

impl Read for TcpTransport {
    /// `Ok(0)` means "nothing yet", never "the peer has gone".
    ///
    /// A closed socket returns `Ok(0)` from `TcpStream::read`, exactly
    /// as a timeout does here, and the session's read loop has no
    /// blocking element of its own: with a live port the read timeout
    /// paces it, but against a closed peer it spun a core for the whole
    /// command timeout, three times over, on every reconnect attempt.
    /// So EOF is reported as an error and the caller treats it as the
    /// link failure it is.
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.stream.read(buf) {
            Ok(0) if !buf.is_empty() => Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("{} closed the connection", self.address),
            )),
            // A read timeout means no bytes yet, not a failure, which
            // is the same contract the serial transport keeps.
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                Ok(0)
            }
            other => other,
        }
    }
}

impl Write for TcpTransport {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.stream.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

impl Transport for TcpTransport {
    fn describe(&self) -> String {
        self.address.clone()
    }
}
