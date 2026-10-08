//! What sensors a platform has beyond those every platform has: on
//! Linux, hwmon and IIO, with the command-line switches that name them;
//! elsewhere, none.  `linux.rs` and `other.rs` each give the same items.

use crate::sensor::ConfigError;
use crate::sensor::Source;

#[cfg_attr(target_os = "linux", path = "linux.rs")]
#[cfg_attr(not(target_os = "linux"), path = "other.rs")]
mod os;

/// The command-line switches naming this platform's own sensors.
pub type Args = os::Args;

/// Every sensor `args` names.
pub fn sources(args: &Args) -> Result<Vec<Box<dyn Source>>, ConfigError> {
    os::sources(args)
}
