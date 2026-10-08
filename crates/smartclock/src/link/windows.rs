//! Windows: there are no Unix sockets.  These types stand in for them,
//! so that `link` names them on every platform, and none can be made.

use std::io;
use std::io::Read;
use std::io::Write;
use std::net::Shutdown;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

/// A connection over a Unix socket, which cannot exist here.
#[derive(Debug)]
pub(super) enum UnixStream {}

/// A Unix socket being listened on, which cannot exist here.
#[derive(Debug)]
pub(super) enum UnixListener {}

impl UnixStream {
    pub(super) fn try_clone(&self) -> io::Result<Self> {
        match *self {}
    }

    pub(super) fn set_read_timeout(&self, _: Option<Duration>) -> io::Result<()> {
        match *self {}
    }

    pub(super) fn set_write_timeout(&self, _: Option<Duration>) -> io::Result<()> {
        match *self {}
    }

    pub(super) fn read_timeout(&self) -> io::Result<Option<Duration>> {
        match *self {}
    }

    pub(super) fn write_timeout(&self) -> io::Result<Option<Duration>> {
        match *self {}
    }

    pub(super) fn shutdown(&self, _: Shutdown) -> io::Result<()> {
        match *self {}
    }
}

impl Read for UnixStream {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        match *self {}
    }
}

impl Write for UnixStream {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        match *self {}
    }

    fn flush(&mut self) -> io::Result<()> {
        match *self {}
    }
}

/// Refused: there is nothing to connect to.
pub(super) fn connect(socket: &Path) -> io::Result<UnixStream> {
    Err(unsupported(socket, "name the service as tcp://HOST:PORT"))
}

/// Refused: there is nothing to bind.
pub(super) fn bind(socket: &Path) -> io::Result<UnixListener> {
    Err(unsupported(socket, "listen on TCP with --listen HOST:PORT"))
}

pub(super) fn accept(listener: &UnixListener) -> io::Result<UnixStream> {
    match *listener {}
}

pub(super) fn path(listener: &UnixListener) -> io::Result<PathBuf> {
    match *listener {}
}

/// No socket, there being none here: a test listens on TCP instead.
pub(super) fn scratch(_: &str) -> Option<PathBuf> {
    None
}

/// That there are no Unix sockets here, and what to do instead.
fn unsupported(socket: &Path, instead: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "{}: there are no Unix sockets here; {instead}",
            socket.display()
        ),
    )
}
