//! Unix: a socket is a file, and its permissions decide who may connect.

use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::path::PathBuf;

/// A connection over a Unix socket.
pub(super) type UnixStream = std::os::unix::net::UnixStream;

/// A Unix socket being listened on.
pub(super) type UnixListener = std::os::unix::net::UnixListener;

/// Connect to the socket at `socket`.
pub(super) fn connect(socket: &Path) -> io::Result<UnixStream> {
    UnixStream::connect(socket)
}

/// Bind a socket at `socket`, owner and group only.
///
/// A socket left behind by a crash would otherwise block the bind, so
/// it is replaced -- but only if nothing answers on it.  Replacing a
/// live service's left that service running and unreachable, and its
/// clients reconnecting to this one.
pub(super) fn bind(socket: &Path) -> io::Result<UnixListener> {
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
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)?;
    // Socket permissions are the whole of the authorization model, so
    // they are set rather than inherited from whatever umask the
    // service happened to start with.  Group access is deliberate: it
    // is how an unprivileged operator runs the monitor.
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o660))?;
    Ok(listener)
}

/// The next client of `listener`.
pub(super) fn accept(listener: &UnixListener) -> io::Result<UnixStream> {
    Ok(listener.accept()?.0)
}

/// Where `listener` is bound.
pub(super) fn path(listener: &UnixListener) -> io::Result<PathBuf> {
    listener
        .local_addr()?
        .as_pathname()
        .map(Path::to_path_buf)
        .ok_or_else(|| io::Error::new(io::ErrorKind::AddrNotAvailable, "an unnamed socket"))
}
