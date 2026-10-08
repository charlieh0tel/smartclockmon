//! Where the programs keep and find things unless told otherwise: one
//! place, so every program agrees with the daemons, and with the units
//! and settings files in `packaging/`, on where that is.

/// The receiver daemons' sockets, one instance per subdirectory:
/// `<RUN_DIR>/<instance>/socket`.
pub const RUN_DIR: &str = "/run/smartclockd";

/// The receiver daemons' logs, one file per receiver.
pub const LOG_DIR: &str = "/var/lib/smartclockd";

/// The sensor service's socket.
pub const SENSOR_SOCKET: &str = "/run/smartclock-sensord/socket";

/// The sensor service's log.
pub const SENSOR_LOG: &str = "/var/lib/smartclock-sensord/sensors.sqlite";
