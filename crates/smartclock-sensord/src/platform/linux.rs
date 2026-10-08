//! Linux: hwmon and IIO sensors, read from sysfs.

mod sysfs;

use crate::sensor::ConfigError;
use crate::sensor::Source;

use sysfs::Interface;

/// The switches naming hwmon and IIO sensors.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// A hwmon sensor, NAME=PATH: its `temp<N>_input` or
    /// `humidity<N>_input` file.  The directory may be a glob matching
    /// one directory, or go through a symlink.  Repeatable; the
    /// environment form is comma-separated.
    #[arg(
        long,
        env = "SMARTCLOCK_SENSORD_HWMON",
        value_delimiter = ',',
        value_name = "NAME=PATH"
    )]
    hwmon: Vec<String>,

    /// An IIO sensor, NAME=CHANNEL: the channel's path less its suffix,
    /// such as `/sys/bus/iio/devices/iio:device0/in_temp`.  As --hwmon
    /// otherwise.
    #[arg(
        long,
        env = "SMARTCLOCK_SENSORD_IIO",
        value_delimiter = ',',
        value_name = "NAME=CHANNEL"
    )]
    iio: Vec<String>,
}

pub(super) fn sources(args: &Args) -> Result<Vec<Box<dyn Source>>, ConfigError> {
    args.hwmon
        .iter()
        .map(|spec| (Interface::Hwmon, spec))
        .chain(args.iio.iter().map(|spec| (Interface::Iio, spec)))
        .map(|(interface, spec)| {
            sysfs::parse(interface, spec).map(|channel| Box::new(channel) as Box<dyn Source>)
        })
        .collect()
}
