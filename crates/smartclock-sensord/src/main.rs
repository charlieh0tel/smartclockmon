//! smartclock-sensord: reads the host's sensors on a period of its own
//! and logs them, whether or not any receiver is attached.
//! `docs/sensors.md` has the design.

use std::collections::HashSet;
use std::fmt::Display;
use std::path::PathBuf;
use std::process::exit;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context as _;
use anyhow::Result;
use clap::CommandFactory as _;
use clap::FromArgMatches as _;
use clap::Parser;
use jiff::Timestamp;
use smartclock::defaults::SENSOR_LISTEN;
use smartclock::defaults::SENSOR_LOG;
use smartclock::defaults::SENSORD;
use smartclock::sensors::DEFAULT_EVERY_S;
use smartclock::sensors::Info;
use smartclock::sensors::Latest;
use smartclock::sensors::Reading;
use smartclock_log::error::Error as LogError;
use smartclock_sensord::log::Log;
use smartclock_sensord::platform;
use smartclock_sensord::sensor::ConfigError;
use smartclock_sensord::sensor::Name;
use smartclock_sensord::sensor::Quantity;
use smartclock_sensord::sensor::Source;
use smartclock_sensord::sensor::check_distinct;
use smartclock_sensord::socket::Answers;
use smartclock_sensord::socket::Shared;
use smartclock_sensord::temper;

/// The exit status for a configuration no retry can fix, which the unit
/// does not restart on: a sensor that cannot be one, no sensors at all,
/// a log written by a newer service.  clap exits 2 for a usage error
/// too.
const CONFIGURATION_ERROR: i32 = 2;

/// The longest read period worth accepting, in seconds: one day.
const LONGEST_EVERY: f64 = 86_400.0;

/// Reads the host's sensors into a log beside the SmartClock receivers'.
#[derive(Debug, Parser)]
#[command(version = smartclock::VERSION)]
struct Cli {
    /// This platform's own sensors.
    #[command(flatten)]
    platform: platform::Args,

    /// A `TEMPerGold` or `TEMPerHUM` USB stick, NAME or NAME=PATH:
    /// with a path, the stick at that hidraw node (Windows: HID device
    /// path), such as a udev link; without one, the first stick found
    /// that no other process holds.  Temperature, and humidity on a
    /// `TEMPerHUM`.  Repeatable; the environment form is comma-separated.
    #[arg(
        long,
        env = "SMARTCLOCK_SENSORD_TEMPER",
        value_delimiter = ',',
        value_name = "NAME[=PATH]"
    )]
    temper: Vec<String>,

    /// How often every sensor is read, in seconds.
    #[arg(long, env = "SMARTCLOCK_SENSORD_EVERY", default_value_t = DEFAULT_EVERY_S)]
    every: f64,

    /// The log.
    #[arg(long, env = "SMARTCLOCK_SENSORD_LOG", default_value = SENSOR_LOG)]
    log: PathBuf,

    /// Where clients connect.
    #[arg(long, env = "SMARTCLOCK_SENSORD_SOCKET", default_value = SENSORD)]
    socket: Option<PathBuf>,

    /// Also listen on TCP at this address, `HOST:PORT`, for clients on
    /// other hosts, which name it as `--sensord tcp://HOST:PORT`.
    ///
    /// Nothing decides who may connect over TCP: anyone who can reach
    /// the address may issue whatever this service allows.
    #[arg(long, env = "SMARTCLOCK_SENSORD_LISTEN", value_parser = smartclock::link::listening_address)]
    listen: Option<String>,
}

/// One source as the loop keeps it, with each quantity it logs.
struct Kept {
    source: Box<dyn Source>,
    logged: Vec<Logged>,
}

/// One quantity a source is logged as.
struct Logged {
    quantity: Quantity,
    /// Its row in the log.
    id: i64,
    /// Its place among the latest readings clients are told.
    shown: usize,
    /// Whether its last read succeeded; `None` before the first.  Its
    /// journal says so only when that changes, not on every read.
    reading: Option<bool>,
}

/// What the loop writes to: the log, the latest readings, and which
/// name and quantity each source has taken.
struct Sinks<'a> {
    log: &'a Log,
    latest: &'a Shared,
    taken: HashSet<(Name, Quantity)>,
}

impl Sinks<'_> {
    /// Log `source` as measuring `quantity` from now on, or say why not.
    fn register(&mut self, source: &dyn Source, quantity: Quantity) -> Result<Logged> {
        if !self.taken.insert((source.name().clone(), quantity)) {
            anyhow::bail!("{} {} is another sensor's", source.name(), quantity.name());
        }
        let id = self.log.sensor_id(source.name(), quantity)?;
        let mut latest = self.latest.lock().unwrap_or_else(PoisonError::into_inner);
        latest.readings.push(Reading {
            name: source.name().to_string(),
            quantity: quantity.name().to_owned(),
            unit: quantity.unit().to_owned(),
            source: source.origin().to_owned(),
            at: None,
            value: None,
            error: None,
        });
        eprintln!(
            "smartclock-sensord: {} {} from {}",
            source.name(),
            quantity.name(),
            source.origin()
        );
        Ok(Logged {
            quantity,
            id,
            shown: latest.readings.len() - 1,
            reading: None,
        })
    }
}

/// The command line, where there are no Unix sockets offering no
/// socket, but a TCP address listened on by default.
fn parse() -> Cli {
    let command = match SENSOR_LISTEN {
        None => Cli::command(),
        Some(address) => Cli::command()
            .mut_arg("socket", |arg| {
                arg.hide(true)
                    .default_value(None)
                    .value_parser(smartclock::link::no_unix_sockets)
            })
            .mut_arg("listen", |arg| arg.default_value(address)),
    };
    Cli::from_arg_matches(&command.get_matches()).unwrap_or_else(|e| e.exit())
}

fn main() {
    let cli = parse();
    if let Err(e) = run(cli) {
        eprintln!("smartclock-sensord: {e:#}");
        exit(1);
    }
}

/// Exit, saying why, with the status the unit does not restart on.
fn refuse(why: impl Display) -> ! {
    eprintln!("smartclock-sensord: {why}");
    exit(CONFIGURATION_ERROR);
}

/// Every source the command line names.
fn sources(cli: &Cli) -> Result<Vec<Box<dyn Source>>, ConfigError> {
    let sticks = cli
        .temper
        .iter()
        .map(|spec| temper::parse(spec).map(|stick| Box::new(stick) as Box<dyn Source>));
    let mut sources = platform::sources(&cli.platform)?;
    sources.extend(sticks.collect::<Result<Vec<_>, _>>()?);
    Ok(sources)
}

fn run(cli: Cli) -> Result<()> {
    if !cli.every.is_finite() || cli.every <= 0.0 || cli.every > LONGEST_EVERY {
        refuse(format!(
            "--every must be a positive number of seconds up to {LONGEST_EVERY}, not {}",
            cli.every
        ));
    }
    let every = Duration::from_secs_f64(cli.every);
    let sources = match sources(&cli) {
        Ok(sources) => sources,
        Err(e) => refuse(e),
    };
    if sources.is_empty() {
        refuse("no sensors are configured; --help lists the kinds there are");
    }
    if let Err(e) = check_distinct(&sources) {
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
        "smartclock-sensord {}: logging {} sensor{} every {} s to {}",
        smartclock::VERSION,
        sources.len(),
        if sources.len() == 1 { "" } else { "s" },
        cli.every,
        cli.log.display()
    );
    let latest: Shared = Arc::new(Mutex::new(Latest {
        every_s: cli.every,
        readings: Vec::new(),
    }));
    let mut sinks = Sinks {
        log: &log,
        latest: &latest,
        taken: HashSet::new(),
    };
    let mut kept = Vec::with_capacity(sources.len());
    for source in sources {
        let logged = source
            .quantities()
            .into_iter()
            .map(|quantity| sinks.register(source.as_ref(), quantity))
            .collect::<Result<_>>()?;
        kept.push(Kept { source, logged });
    }

    let listeners = smartclock::server::listen_all(cli.socket.as_deref(), cli.listen.as_deref())?;
    let answers = Arc::new(Answers {
        info: Info {
            version: smartclock::VERSION.to_owned(),
            every_s: cli.every,
            log: cli.log.display().to_string(),
        },
        latest: Arc::clone(&latest),
    });
    std::thread::Builder::new()
        .name("smartclock-sensord-socket".to_owned())
        .spawn(move || smartclock::server::serve(listeners, "smartclock-sensord", answers))
        .context("spawning the socket server")?;

    let mut next = Instant::now();
    loop {
        for one in &mut kept {
            pass(&mut sinks, one);
        }
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

/// Read one source once, log what read, and tell clients.
fn pass(sinks: &mut Sinks<'_>, kept: &mut Kept) {
    let at = Timestamp::now();
    let name = kept.source.name().clone();
    match kept.source.read() {
        Ok(measures) => {
            let device = kept.source.device();
            for measure in measures {
                let found = kept
                    .logged
                    .iter()
                    .position(|logged| logged.quantity == measure.quantity);
                let index = match found {
                    Some(index) => index,
                    // Measured but not known to be until now: logged from
                    // here on, once.
                    None => match sinks.register(kept.source.as_ref(), measure.quantity) {
                        Ok(logged) => {
                            kept.logged.push(logged);
                            kept.logged.len() - 1
                        }
                        Err(e) => {
                            eprintln!("smartclock-sensord: {name}: not logged: {e:#}");
                            continue;
                        }
                    },
                };
                let logged = &mut kept.logged[index];
                show(sinks.latest, logged.shown, |shown| {
                    shown.at = Some(at);
                    shown.value = Some(measure.value);
                    shown.error = None;
                });
                if logged.reading != Some(true) {
                    eprintln!(
                        "smartclock-sensord: {name} {}: reading, {} {}",
                        measure.quantity.name(),
                        measure.value,
                        measure.quantity.unit()
                    );
                }
                logged.reading = Some(true);
                let written = sinks
                    .log
                    .note_source(logged.id, at, kept.source.origin(), device.as_deref())
                    .and_then(|()| sinks.log.record(logged.id, at, measure.value));
                if let Err(e) = written {
                    eprintln!(
                        "smartclock-sensord: {name} {}: not logged: {e}",
                        measure.quantity.name()
                    );
                }
            }
        }
        Err(e) => {
            for logged in &mut kept.logged {
                show(sinks.latest, logged.shown, |shown| {
                    shown.error = Some(e.to_string());
                });
                if logged.reading != Some(false) {
                    eprintln!(
                        "smartclock-sensord: {name} {}: no reading: {e}",
                        logged.quantity.name()
                    );
                }
                logged.reading = Some(false);
            }
        }
    }
}

/// Change what clients are told of one reading.
fn show(latest: &Shared, shown: usize, change: impl FnOnce(&mut Reading)) {
    let mut latest = latest.lock().unwrap_or_else(PoisonError::into_inner);
    change(&mut latest.readings[shown]);
}
