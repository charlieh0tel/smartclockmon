//! Reads the host's hwmon and IIO sensors -- the room's temperature,
//! humidity and pressure -- into a log beside the receivers'.
//! `docs/sensors.md` has the design.  Linux only: hwmon and IIO are
//! Linux's, and elsewhere there would be nothing to read.

#[cfg(not(target_os = "linux"))]
compile_error!("smartclock-sensord reads Linux's hwmon and IIO, and builds only on Linux");

pub mod log;
pub mod sensor;
pub mod socket;
