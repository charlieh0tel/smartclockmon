//! Where the programs keep and find things unless told otherwise: one
//! place, so every program agrees with the daemons, and with the units
//! and settings files in `packaging/`, on where that is.
//!
//! Where things are differs between Unix and Windows, and that is all
//! here; how a connection is made differs too, and is in `link`.  On
//! Unix a daemon's clients find it by its socket under [`RUN_DIR`].
//! Windows has no Unix sockets, so a daemon listens on [`TCP_LISTEN`]
//! and its clients ask [`DAEMON`]; a second daemon there needs a port of
//! its own, and its clients told which.  The sensor service likewise
//! listens on [`SENSOR_LISTEN`], and its clients ask [`SENSORD`].

#[cfg(unix)]
mod os {
    pub(super) const RUN_DIR: &str = "/run/smartclockd";
    pub(super) const LOG_DIR: &str = "/var/lib/smartclockd";
    pub(super) const SENSORD: &str = "/run/smartclock-sensord/socket";
    pub(super) const SENSOR_LOG: &str = "/var/lib/smartclock-sensord/sensors.sqlite";
    pub(super) const SENSOR_LISTEN: Option<&str> = None;
    pub(super) const TCP_LISTEN: Option<&str> = None;
    pub(super) const DAEMON: Option<&str> = None;
}

/// Where a receiver daemon listens where there are no Unix sockets.
#[cfg(windows)]
macro_rules! tcp_listen {
    () => {
        "127.0.0.1:9978"
    };
}

/// Where the sensor service listens where there are no Unix sockets.
#[cfg(windows)]
macro_rules! sensor_listen {
    () => {
        "127.0.0.1:9977"
    };
}

#[cfg(windows)]
mod os {
    pub(super) const RUN_DIR: &str = r"C:\ProgramData\smartclockmon\run";
    pub(super) const LOG_DIR: &str = r"C:\ProgramData\smartclockmon\log";
    pub(super) const SENSORD: &str = concat!("tcp://", sensor_listen!());
    pub(super) const SENSOR_LOG: &str = r"C:\ProgramData\smartclockmon\log\sensors.sqlite";
    pub(super) const SENSOR_LISTEN: Option<&str> = Some(sensor_listen!());
    pub(super) const TCP_LISTEN: Option<&str> = Some(tcp_listen!());
    pub(super) const DAEMON: Option<&str> = Some(concat!("tcp://", tcp_listen!()));
}

/// The receiver daemons' sockets, one instance per subdirectory:
/// `<RUN_DIR>/<instance>/socket`.
pub const RUN_DIR: &str = os::RUN_DIR;

/// The receiver daemons' logs, one file per receiver.
pub const LOG_DIR: &str = os::LOG_DIR;

/// Where the sensor service's clients find it: its socket, or where
/// there are no Unix sockets its TCP address.
pub const SENSORD: &str = os::SENSORD;

/// The sensor service's log.
pub const SENSOR_LOG: &str = os::SENSOR_LOG;

/// Where a receiver daemon listens on TCP when told nowhere else; there
/// is one exactly where there are no Unix sockets.
pub const TCP_LISTEN: Option<&str> = os::TCP_LISTEN;

/// The receiver daemon a client asks when told none; there is one
/// exactly where there are no Unix sockets.
pub const DAEMON: Option<&str> = os::DAEMON;

/// Where the sensor service listens on TCP when told nowhere else;
/// there is one exactly where there are no Unix sockets.
pub const SENSOR_LISTEN: Option<&str> = os::SENSOR_LISTEN;
