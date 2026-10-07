//! smartclock-sensord: reads the host's hwmon and IIO sensors on a
//! period of its own and logs them, whether or not any receiver is
//! attached.  `docs/sensors.md` has the design.

use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context as _;
use anyhow::Result;
use clap::Parser;
use jiff::Timestamp;
use smartclock_log::error::Error as LogError;
use smartclock_sensord::log::Log;
use smartclock_sensord::sensor;
use smartclock_sensord::sensor::Interface;
use smartclock_sensord::sensor::Sensor;

/// The exit status for a configuration no retry can fix, which the unit
/// does not restart on: a sensor that cannot be one, no sensors at all,
/// a log written by a newer service.  clap exits 2 for a usage error
/// too.
const CONFIGURATION_ERROR: i32 = 2;

/// The longest read period worth accepting, in seconds.
const LONGEST_EVERY: f64 = 86_400.0;

/// Reads the host's sensors into a log beside the SmartClock receivers'.
#[derive(Debug, Parser)]
#[command(version = smartclock::VERSION)]
struct Cli {
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

    /// How often every sensor is read, in seconds.
    #[arg(long, env = "SMARTCLOCK_SENSORD_EVERY", default_value_t = 10.0)]
    every: f64,

    /// The log.
    #[arg(
        long,
        env = "SMARTCLOCK_SENSORD_LOG",
        default_value = "/var/lib/smartclock-sensord/sensors.sqlite"
    )]
    log: PathBuf,
}

/// One sensor as the loop keeps it.
struct Kept {
    sensor: Sensor,
    /// Its row in the log.
    id: i64,
    /// Whether its last read succeeded; `None` before the first.  Its
    /// journal says so only when that changes, not on every read.
    reading: Option<bool>,
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("smartclock-sensord: {e:#}");
        std::process::exit(1);
    }
}

/// Exit, saying why, with the status the unit does not restart on.
fn refuse(why: impl std::fmt::Display) -> ! {
    eprintln!("smartclock-sensord: {why}");
    std::process::exit(CONFIGURATION_ERROR);
}

fn run(cli: Cli) -> Result<()> {
    if !cli.every.is_finite() || cli.every <= 0.0 || cli.every > LONGEST_EVERY {
        refuse(format!(
            "--every must be a positive number of seconds up to {LONGEST_EVERY}, not {}",
            cli.every
        ));
    }
    let every = Duration::from_secs_f64(cli.every);
    let configured = cli
        .hwmon
        .iter()
        .map(|spec| (Interface::Hwmon, spec))
        .chain(cli.iio.iter().map(|spec| (Interface::Iio, spec)));
    let sensors: Vec<Sensor> = match configured
        .map(|(interface, spec)| sensor::parse(interface, spec))
        .collect::<Result<_, _>>()
    {
        Ok(sensors) => sensors,
        Err(e) => refuse(e),
    };
    if sensors.is_empty() {
        refuse("no sensors are configured: set SMARTCLOCK_SENSORD_HWMON or SMARTCLOCK_SENSORD_IIO");
    }
    if let Err(e) = sensor::check_distinct(&sensors) {
        refuse(e);
    }

    if let Some(parent) = cli.log.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let log = match Log::open(&cli.log, every) {
        Ok(log) => log,
        Err(e @ LogError::NewerSchema { .. }) => refuse(format!("{}: {e}", cli.log.display())),
        Err(e) => return Err(e).with_context(|| format!("opening {}", cli.log.display())),
    };
    eprintln!(
        "smartclock-sensord {}: logging {} sensors every {} s to {}",
        smartclock::VERSION,
        sensors.len(),
        cli.every,
        cli.log.display()
    );
    let mut kept = Vec::with_capacity(sensors.len());
    for sensor in sensors {
        let id = log.sensor_id(&sensor)?;
        eprintln!(
            "smartclock-sensord: {} {} from {}",
            sensor.name,
            sensor.quantity.name(),
            sensor.source
        );
        kept.push(Kept {
            sensor,
            id,
            reading: None,
        });
    }

    let mut next = Instant::now();
    loop {
        pass(&log, &mut kept);
        next += every;
        let now = Instant::now();
        // A pass that ran past its slot -- a suspended host, a bus that
        // stalled -- starts the schedule again from now rather than
        // reading in a burst to catch up.
        if next < now {
            next = now;
        }
        std::thread::sleep(next - now);
    }
}

/// Read every sensor once and log what read.
fn pass(log: &Log, kept: &mut [Kept]) {
    for one in kept {
        let at = Timestamp::now();
        let name = format!("{} {}", one.sensor.name, one.sensor.quantity.name());
        match sensor::read(&one.sensor) {
            Ok(value) => {
                if one.reading != Some(true) {
                    eprintln!(
                        "smartclock-sensord: {name}: reading, {value} {}",
                        one.sensor.quantity.unit()
                    );
                }
                one.reading = Some(true);
                let device = sensor::device(&one.sensor);
                let written = log
                    .note_source(one.id, at, &one.sensor.source, device.as_deref())
                    .and_then(|()| log.record(one.id, at, value));
                if let Err(e) = written {
                    eprintln!("smartclock-sensord: {name}: not logged: {e}");
                }
            }
            Err(e) => {
                if one.reading != Some(false) {
                    eprintln!("smartclock-sensord: {name}: no reading: {e}");
                }
                one.reading = Some(false);
            }
        }
    }
}
