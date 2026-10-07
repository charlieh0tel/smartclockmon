//! Reads the host's hwmon and IIO sensors -- the room's temperature,
//! humidity and pressure -- into a log beside the receivers'.
//! `docs/sensors.md` has the design.

pub mod log;
pub mod sensor;
pub mod socket;
