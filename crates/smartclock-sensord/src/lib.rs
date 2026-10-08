//! Reads the host's sensors -- the room's temperature, humidity and
//! pressure -- into a log beside the receivers'.  `docs/sensors.md` has
//! the design.

pub mod log;
pub mod sensor;
pub mod socket;
pub mod sysfs;
pub mod temper;
