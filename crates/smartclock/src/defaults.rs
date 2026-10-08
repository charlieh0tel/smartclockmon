//! Where the programs keep and find things unless told otherwise: one
//! place, so every program agrees with the daemons, and with the units
//! and settings files in `packaging/`, on where that is.
//!
//! What differs between Unix and Windows is here and nowhere else.  On
//! Unix a daemon's clients find it by its socket under [`RUN_DIR`].
//! Windows has no Unix sockets, so a daemon listens on [`TCP_LISTEN`]
//! and its clients ask [`DAEMON`]; a second daemon there needs a port of
//! its own, and its clients told which.

#[cfg(unix)]
mod os {
    pub(super) const RUN_DIR: &str = "/run/smartclockd";
    pub(super) const LOG_DIR: &str = "/var/lib/smartclockd";
    pub(super) const SENSOR_SOCKET: &str = "/run/smartclock-sensord/socket";
    pub(super) const SENSOR_LOG: &str = "/var/lib/smartclock-sensord/sensors.sqlite";
    pub(super) const TCP_LISTEN: Option<&str> = None;
    pub(super) const DAEMON: Option<&str> = None;
}

#[cfg(windows)]
mod os {
    pub(super) const RUN_DIR: &str = r"C:\ProgramData\smartclockmon\run";
    pub(super) const LOG_DIR: &str = r"C:\ProgramData\smartclockmon\log";
    /// Never served, since the sensor service is Linux's, but named so
    /// that a client asking for it finds nothing.
    pub(super) const SENSOR_SOCKET: &str = r"C:\ProgramData\smartclockmon\run\sensord";
    pub(super) const SENSOR_LOG: &str = r"C:\ProgramData\smartclockmon\log\sensors.sqlite";
    pub(super) const TCP_LISTEN: Option<&str> = Some("127.0.0.1:9978");
    pub(super) const DAEMON: Option<&str> = Some("tcp://127.0.0.1:9978");
}

/// The receiver daemons' sockets, one instance per subdirectory:
/// `<RUN_DIR>/<instance>/socket`.
pub const RUN_DIR: &str = os::RUN_DIR;

/// The receiver daemons' logs, one file per receiver.
pub const LOG_DIR: &str = os::LOG_DIR;

/// The sensor service's socket.
pub const SENSOR_SOCKET: &str = os::SENSOR_SOCKET;

/// The sensor service's log.
pub const SENSOR_LOG: &str = os::SENSOR_LOG;

/// Where a receiver daemon listens on TCP when told nowhere at all.
pub const TCP_LISTEN: Option<&str> = os::TCP_LISTEN;

/// The receiver daemon a client asks when told none.
pub const DAEMON: Option<&str> = os::DAEMON;
