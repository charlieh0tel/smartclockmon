//! One-shot queries, transcript capture, and command table probing.
//!
//! Talks to the receiver directly.  `smartclockd` holds the serial port
//! open once it exists, so it must be stopped before using this.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context as _;
use anyhow::Result;
use clap::Parser;
use clap::Subcommand;
use jiff::Timestamp;
use jiff::Zoned;
#[cfg(unix)]
use signal_hook::consts::SIGINT;
use smartclock::attach::answering;
use smartclock::client::Daemon;
use smartclock::client::Filed;
use smartclock::command::Class;
use smartclock::command::Dialect;
use smartclock::console;
use smartclock::console::Progress;
use smartclock::console::Region;
use smartclock::control::forbidden;
use smartclock::device::Device;
use smartclock::device::dialect_for;
use smartclock::error::Error;
use smartclock::parse;
use smartclock::rollover::ReceiverDate;
use smartclock::session::Config;
use smartclock::session::Session;
use smartclock::transport;
use smartclock::transport::Transport;
use smartclock::transport::serial::Settings;
use smartclock::transport::tee::TeeTransport;
use smartclock::types::BaudRate;
use smartclock::types::Framing;
use smartclock::types::Seconds;
use smartclock::wire::Reading;
use smartclock_log::reader::Fact;
use smartclock_log::reader::Log;

use crate::flash::FlashArgs;

#[derive(Parser)]
#[command(about, version = smartclock::VERSION)]
struct Cli {
    /// Serial device, or `tcp://host:port` for a receiver on the
    /// network or the simulator.  Prefer a /dev/serial/by-id/... path
    /// for a local one, which survives USB re-enumeration.
    ///
    /// Required, and deliberately without a default: guessing at a
    /// path means talking to whatever is on it.
    #[arg(long, env = "SMARTCLOCK_DEVICE", global = true)]
    device: Option<String>,

    /// Bits per second.  The receiver stores this setting, so it is not
    /// necessarily the 9600 factory default.  Checked against the rates a
    /// port can be opened at: an unsupported rate does not fail on open, it
    /// produces garbage that looks like a dead receiver.
    #[arg(long, default_value_t = 19200, global = true)]
    baud: u32,

    /// Character framing, `8N1` or `7O1`.  The Z3801A's is fixed at 7O1.
    #[arg(long, default_value = "8N1", global = true)]
    framing: Framing,

    /// Seconds to wait for a prompt.
    #[arg(long, default_value = "5", value_parser = seconds, global = true)]
    timeout: Duration,

    /// Ask a running smartclockd instead of opening the port.
    ///
    /// The daemon holds the serial port for as long as it runs, so a
    /// direct-mode tool has to be given `--device` with the daemon
    /// stopped.  This talks to the daemon over its socket instead,
    /// which leaves the logging running and is gated by whatever flags
    /// the daemon was started with.
    #[arg(long, conflicts_with_all = ["device", "capture"], global = true)]
    socket: Option<PathBuf>,

    /// Record the whole exchange to this JSONL transcript, a new file:
    /// an existing one is refused, not overwritten.
    #[arg(long, global = true)]
    capture: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Send commands and print their replies.
    Query {
        /// SCPI commands to send, in order.
        #[arg(required = true)]
        commands: Vec<String>,
    },
    /// Send every read-only command in the table and report which ones
    /// the receiver actually answers.  This is how table entries earn
    /// their `verified` flag.
    Probe {
        /// Which command tree to probe, `hp58503` or `z3801`.  By default,
        /// the one the receiver's `*IDN?` model names.
        #[arg(long)]
        dialect: Option<String>,
    },
    /// Report oscillator and holdover health in one pass.
    Diagnose,
    /// Print the command table as Markdown.  Sends nothing.
    Commands,
    /// Try each command in a file and report which the receiver knows.
    ///
    /// For finding commands the manuals do not document.  Only sends
    /// what is given, so the file must contain queries; an unknown
    /// header comes back -113 and changes nothing.
    Sweep {
        /// One SCPI command per line.  Blank lines and # are skipped.
        #[arg(long)]
        from: PathBuf,
    },
    /// Read memory through the pForth debug console, entering it with
    /// `:SYSTem:LANGuage "PFORTH"` if the port is not already there
    /// (docs/firmware/console.md, "Reading memory through it").  Defines one word
    /// in the console's RAM and writes nothing else.  Afterwards the
    /// port is returned to SCPI with the console's `halt`, or failing
    /// that through the primary's exit into the installer and
    /// `:SYSTem:LANGuage "PRIMARY"` (docs/firmware/console.md, "Leaving it");
    /// nothing is erased or programmed.
    ReadMemory {
        /// First address, as 0x-prefixed hex or decimal.
        #[arg(long, value_parser = address)]
        from: u32,
        /// How many bytes.
        #[arg(long, value_parser = address)]
        length: u32,
        #[command(flatten)]
        to: ReadTo,
    },
    /// Check a firmware image and the receiver it is for; with
    /// `--write`, erase and program the receiver's flash through its
    /// installer, check that the new primary boots, and read the whole
    /// flash back (docs/firmware/restart.md, "The flasher").  Stop the port's
    /// daemon first.  The only command that erases or programs.
    Flash(FlashArgs),
    /// Read the whole flash, 512 KiB from address 0, as `read-memory`
    /// does.
    ReadFlash {
        #[command(flatten)]
        to: ReadTo,
    },
    /// Read the whole EEPROM, 8 KiB from `0x400000`, as `read-memory`
    /// does.
    ReadEeprom {
        #[command(flatten)]
        to: ReadTo,
    },
    /// Write a note to the log of the daemon's receiver, such as "added
    /// a 20 dB LNA".  Needs `--socket`; nothing is sent to the receiver.
    Note {
        /// What it says; the words are joined with spaces.
        #[arg(required = true)]
        text: Vec<String>,
        /// When it happened, as RFC 3339 with an offset; now by default.
        #[arg(long)]
        at: Option<Timestamp>,
    },
    /// Record a fact about the daemon's receiver, such as `ocxo.serial
    /// 1234`, and a note saying so.  Needs `--socket`; nothing is sent
    /// to the receiver.
    Fact {
        /// What it is about, one word, such as `ocxo.serial`.
        key: String,
        /// Its value; the words are joined with spaces.
        #[arg(required = true)]
        value: Vec<String>,
        /// When it became true, as RFC 3339 with an offset; now by
        /// default.
        #[arg(long)]
        since: Option<Timestamp>,
    },
    /// List the host's sensors and their latest readings, from
    /// smartclock-sensord.  Needs neither a receiver nor its daemon.
    Sensors {
        /// The sensor service's socket.
        #[arg(long, default_value = "/run/smartclock-sensord/socket")]
        sensor_socket: PathBuf,
    },
}

/// Where a console read goes and what happens after it, shared by the
/// memory-reading commands.
#[derive(Debug, clap::Args)]
struct ReadTo {
    /// Where to write what was read.
    #[arg(long)]
    out: PathBuf,
    /// An image of the same length to check the read against, from its
    /// first byte.  Any difference makes the command fail.
    #[arg(long)]
    compare: Option<PathBuf>,
    /// Leave the port at the console instead of returning it to
    /// SCPI.  Leave it with `halt` by hand, or power cycle.
    #[arg(long)]
    stay_in_console: bool,
}

/// The flash, as every model maps it: the boot and primary images
/// together (docs/firmware/restart.md, "The installer").
const FLASH: (u32, u32) = (0, 0x80000);

/// The EEPROM behind chip select 9 (docs/firmware/console.md, "Reading memory
/// through it").
const EEPROM: (u32, u32) = (0x40_0000, 0x2000);

/// The range and destination of a command that reads memory through
/// the console, or `None` for any other command.
fn console_read(command: &Command) -> Option<(u32, u32, &ReadTo)> {
    match command {
        Command::ReadMemory { from, length, to } => Some((*from, *length, to)),
        Command::ReadFlash { to } => Some((FLASH.0, FLASH.1, to)),
        Command::ReadEeprom { to } => Some((EEPROM.0, EEPROM.1, to)),
        _ => None,
    }
}

/// Print a console read's progress.
fn report(progress: Progress) {
    match progress {
        Progress::Entering => {
            eprintln!("not at the pForth prompt; sending :SYSTem:LANGuage \"PFORTH\"");
        }
        Progress::Read {
            done,
            elapsed,
            differing,
        } => eprintln!(
            "{done:#x} read, {} s, {differing} differing chunks",
            elapsed.as_secs()
        ),
    }
}

/// A `--timeout`: a positive, finite number of seconds.
///
/// Parsed here rather than converted later, where `Duration`'s own
/// conversion panics on a negative or non-finite value and a zero
/// timeout gives up before anything can answer.
fn seconds(text: &str) -> std::result::Result<Duration, String> {
    let value: f64 = text
        .parse()
        .map_err(|e| format!("{text} is not a number of seconds: {e}"))?;
    Duration::try_from_secs_f64(value)
        .ok()
        .filter(|d| !d.is_zero())
        .ok_or_else(|| format!("{text} is not a positive number of seconds"))
}

/// A `--from` or `--length`: 0x-prefixed hex, or decimal.
fn address(text: &str) -> std::result::Result<u32, String> {
    let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => text.parse(),
    };
    parsed.map_err(|e| format!("{text} is not an address: {e}"))
}

mod flash;

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Reading the table needs no receiver, so this runs before the port
    // is opened rather than failing for want of hardware.
    if matches!(cli.command, Command::Commands) {
        print!("{}", smartclock::matrix::markdown());
        return Ok(());
    }
    if let Command::Sensors { sensor_socket } = &cli.command {
        return sensors(sensor_socket);
    }

    if let Some(socket) = cli.socket.clone() {
        return through_daemon(&socket, &cli.command);
    }
    if matches!(cli.command, Command::Note { .. } | Command::Fact { .. }) {
        anyhow::bail!("notes and facts are written to the log by the daemon; give --socket");
    }
    // Before the port is opened, so a refused list sends nothing at
    // all rather than the commands ahead of the refused one.  What is
    // checked is what is sent: a sweep file is read here once.
    let checked = typed(&cli.command)?;
    refuse_forbidden(&checked)?;

    let baud = BaudRate::new(cli.baud).with_context(|| {
        let supported = BaudRate::ALL.map(|b| b.to_string()).join(", ");
        format!(
            "{} is not a rate the receiver supports ({supported})",
            cli.baud
        )
    })?;
    // clap will not let a global argument be required, so the check is
    // here.  It is still not optional: guessing at a path means talking
    // to whatever is on it.
    // It opens and probes the port itself, at its own timeout, and
    // needs no device to inspect an image.
    if let Command::Flash(args) = &cli.command {
        return flash::run(
            args,
            cli.device.as_deref(),
            baud,
            cli.framing,
            cli.capture.as_deref(),
        );
    }
    let device = cli.device.clone().context(
        "--device is required and has no default; SMARTCLOCK_DEVICE works too. \
         Try: ls -l /dev/serial/by-id/",
    )?;
    let settings = Settings {
        path: device.clone(),
        baud,
        framing: cli.framing,
        read_timeout: Duration::from_millis(250),
    };
    let config = Config {
        timeout: cli.timeout,
        ..Config::default()
    };
    // The console has its own prompt, where `*IDN?` means nothing, so
    // only SCPI is looked for at other line settings.
    let settings = if console_read(&cli.command).is_some() {
        settings
    } else {
        let (found, report) = answering(&settings, &config)
            .with_context(|| format!("no receiver answered on {}", settings.path))?;
        if let Some(report) = report {
            eprintln!("{report}");
        }
        found
    };
    // Long enough for the status screen at the rate found.
    let config = config.at_rate(settings.baud);
    // Created before the port is opened for the run, so an existing
    // file stops it before the run sends anything.  The line-settings
    // probe above has already spoken to the receiver, unrecorded.
    let capture = cli.capture.as_deref().map(transcript).transpose()?;
    let port = transport::open(&settings)
        .with_context(|| format!("opening {device} at {} baud", settings.baud))?;
    // Capture wraps the port, so a recording covers the sync exchange
    // and the console reads too, not just the commands a subcommand
    // issues.
    let mut port: Box<dyn Transport + Send> = match capture {
        Some(file) => Box::new(TeeTransport::new(port, file)),
        None => port,
    };

    // The console has its own prompt, so this skips the SCPI session.
    if let Some((from, length, to)) = console_read(&cli.command) {
        let region = Region::new(from, length)?;
        let ReadTo {
            out,
            compare,
            stay_in_console,
        } = to;
        let expected = compare
            .as_ref()
            .map(|path| std::fs::read(path).with_context(|| format!("reading {}", path.display())))
            .transpose()?;
        // Checked before anything is sent: a comparison image of another
        // length cannot say whether the read matches.
        if let (Some(path), Some(bytes)) = (compare, &expected) {
            anyhow::ensure!(
                bytes.len() == length as usize,
                "{} is {:#x} bytes, but {length:#x} are to be read; nothing was sent",
                path.display(),
                bytes.len()
            );
        }
        let mut file = File::create(out).with_context(|| format!("creating {}", out.display()))?;
        let summary = if *stay_in_console {
            console::read_memory(
                &mut port,
                region,
                &mut file,
                expected.as_deref(),
                report,
                &*stop_on_interrupt()?,
            )?
        } else {
            let (summary, identity) = console::read_and_return(
                port,
                region,
                &mut file,
                expected.as_deref(),
                report,
                &*stop_on_interrupt()?,
                config,
            )?;
            eprintln!("back at SCPI: {identity}");
            summary
        };
        eprintln!(
            "read {length:#x} bytes from {from:#x} into {} in {} s",
            out.display(),
            summary.took.as_secs()
        );
        if let Some(path) = compare {
            for chunk in &summary.differing {
                eprintln!("differs from {} in the 1 KB at {chunk:#x}", path.display());
            }
            anyhow::ensure!(
                summary.differing.is_empty(),
                "{} of the read's 1 KB blocks differ from {}",
                summary.differing.len(),
                path.display()
            );
            eprintln!("identical to {}", path.display());
        }
        return Ok(());
    }

    run(Session::new(port, config), &cli.command, &checked)
}

/// The exit status after a second Ctrl-C: 128 plus SIGINT's number.
const EXIT_INTERRUPTED: i32 = 130;

/// Set by Ctrl-C during a flash or a console read.
static STOP: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// Whether Ctrl-C has asked the work to stop.
fn stopping() -> bool {
    STOP.get().is_some_and(|stop| stop.load(Ordering::Relaxed))
}

/// Make Ctrl-C ask the work to stop at its next record or chunk, so the
/// port is left somewhere it can be recovered from, and a second
/// Ctrl-C exit at once.  For the commands that check the flag; the rest
/// keep Ctrl-C's default.
fn stop_on_interrupt() -> Result<Arc<AtomicBool>> {
    if let Some(stop) = STOP.get() {
        return Ok(Arc::clone(stop));
    }
    let stop = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    {
        // Registered first, so it sees the flag as the previous Ctrl-C
        // left it: set means this is the second.
        signal_hook::flag::register_conditional_shutdown(
            SIGINT,
            EXIT_INTERRUPTED,
            Arc::clone(&stop),
        )?;
        signal_hook::flag::register(SIGINT, Arc::clone(&stop))?;
    }
    Ok(Arc::clone(STOP.get_or_init(|| stop)))
}

/// A new transcript file at `path`.  An existing file is refused rather
/// than overwritten: a transcript is evidence, and the one written last
/// is not always the one that mattered.
fn transcript(path: &Path) -> Result<File> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("creating transcript {}", path.display()))?;
    eprintln!("Recording the exchange to {}.", path.display());
    Ok(file)
}

/// Carry out `command`, sending `checked` where it is one that sends
/// commands as given.
fn run<T: Transport>(mut session: Session<T>, command: &Command, checked: &[String]) -> Result<()> {
    session
        .sync()
        .with_context(|| format!("no prompt from {}", session.describe()))?;

    // diagnose builds a Device, which takes the session, so it is
    // handled before the borrowing arms below.
    if matches!(command, Command::Diagnose) {
        return diagnose(session);
    }
    let session = &mut session;

    let result = match command {
        // A closure, so a failed query still reaches the strays below.
        Command::Query { .. } => (|| {
            for one in checked {
                let reply = session
                    .query(one)
                    .with_context(|| format!("sending {one}"))?;
                for line in &reply.lines {
                    println!("{line}");
                }
            }
            Ok(())
        })(),
        Command::Probe { dialect } => probe(session, dialect.as_deref()),
        Command::Sweep { .. } => sweep(session, checked),
        Command::Diagnose => unreachable!("handled above, since it takes the session"),
        Command::ReadMemory { .. }
        | Command::ReadFlash { .. }
        | Command::ReadEeprom { .. }
        | Command::Flash(_) => unreachable!("handled before the session is opened"),
        Command::Note { .. } | Command::Fact { .. } => {
            unreachable!("refused without --socket, before the port is opened")
        }
        // Handled before the port is opened.
        Command::Commands => Ok(()),
        Command::Sensors { .. } => unreachable!("handled before the port is opened"),
    };
    // Errors read off the queue so that no command was judged by them
    // (docs/protocol.md, "The error prompt"): the receiver raised them,
    // so they are shown rather than dropped.
    for stray in session.take_stray_errors() {
        eprintln!(
            "the receiver also held: {},\"{}\"",
            stray.code, stray.message
        );
    }
    result
}

/// The commands a subcommand sends as given by the operator, rather
/// than from the command table.
fn typed(command: &Command) -> Result<Vec<String>> {
    Ok(match command {
        Command::Query { commands } => commands.clone(),
        Command::Sweep { from } => candidates(from)?,
        Command::Probe { .. }
        | Command::Diagnose
        | Command::Commands
        | Command::ReadMemory { .. }
        | Command::ReadFlash { .. }
        | Command::ReadEeprom { .. }
        | Command::Flash(_)
        | Command::Note { .. }
        | Command::Fact { .. }
        | Command::Sensors { .. } => Vec::new(),
    })
}

/// Refuse the whole list if any command in it is one no direct-mode
/// tool sends; see [`smartclock::control::forbidden`].
fn refuse_forbidden(commands: &[String]) -> Result<()> {
    for scpi in commands {
        if let Some(which) = forbidden(scpi) {
            anyhow::bail!(
                "refusing to send {scpi:?}: it is {which}, which this tool never sends to \
                 the receiver directly; nothing was sent"
            );
        }
    }
    Ok(())
}

/// The commands in a sweep file: one per line, blank lines and `#`
/// skipped.
fn candidates(from: &PathBuf) -> Result<Vec<String>> {
    let text =
        std::fs::read_to_string(from).with_context(|| format!("reading {}", from.display()))?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect())
}

/// Send each candidate and report what the receiver makes of it.
fn sweep<T: Transport>(session: &mut Session<T>, candidates: &[String]) -> Result<()> {
    let mut known = Vec::new();
    let (mut unknown, mut refused, mut failed) = (0usize, 0usize, 0usize);
    for scpi in candidates {
        match session.query(scpi) {
            Ok(reply) => {
                let value = reply.lines.join(" | ");
                println!("FOUND    {scpi:<52} {value}");
                known.push((scpi.as_str(), value));
            }
            // -113 is the whole point of the sweep: the header does not
            // exist, so the command is not there.
            Err(Error::Device { code: -113, .. }) => unknown += 1,
            Err(Error::Device { code, message }) => {
                refused += 1;
                println!("EXISTS   {scpi:<52} {code}: {message}");
            }
            Err(e) => {
                failed += 1;
                println!("FAILED   {scpi:<52} {e}");
                session.sync().context("resynchronizing after a failure")?;
            }
        }
        std::io::stdout().flush().ok();
    }

    println!(
        "\n{} candidates: {} answered, {refused} exist but declined, {unknown} undefined, {failed} failed",
        candidates.len(),
        known.len()
    );
    Ok(())
}

/// Print what the receiver says about its oscillator and its lock.
///
/// Built on `Device` rather than on literal SCPI.  It used to send
/// eighteen 58503A spellings directly, so the tool most likely to be
/// pointed at an unfamiliar receiver was the one that ignored the
/// command table and asked a Z3801A in the wrong dialect.
fn diagnose<T: Transport>(session: Session<T>) -> Result<()> {
    let mut device = Device::open(session).context("identifying the receiver")?;
    let id = device.identity().clone();
    println!(
        "{} {}  serial {}  firmware {}",
        id.manufacturer, id.model, id.serial, id.firmware
    );
    line("dialect", device.dialect().name());

    println!("\nLock");
    show("mode", device.mode().map(|m| m.to_string()));
    show(
        "waiting to recover",
        device.holdover_waiting().map(|w| w.to_string()),
    );
    show("TFOM", device.tfom().map(|v| v.to_string()));
    show("FFOM", device.ffom().map(|v| v.to_string()));
    show(
        "1 PPS TI",
        device
            .time_interval()
            .map(|v| absent_or(v, |v| v.to_string())),
    );

    println!("\nOscillator");
    match device.efc() {
        Ok(efc) => line(
            "EFC",
            &format!(
                "{efc}  ({:.0}% of tuning range used)",
                efc.range_used() * 100.0
            ),
        ),
        Err(e) => line("EFC", &format!("unavailable: {e}")),
    }
    show(
        "internal temp",
        device
            .temperature()
            .map(|v| absent_or(v, |v| format!("{v:.2} C"))),
    );
    show(
        "oven current",
        device
            .oven_current()
            .map(|v| absent_or(v, |v| format!("{v:.1}"))),
    );
    show(
        "current coeff",
        device
            .oven_tempco()
            .map(|v| absent_or(v, |v| format!("{v:.2} EFC counts per unit of oven current"))),
    );
    show(
        "EFC raw",
        device.efc_dac().map(|v| absent_or(v, |v| v.to_string())),
    );
    match device.hardware_condition() {
        Ok(condition) if condition.is_healthy() => {
            line(
                "hardware",
                &format!("no faults (register {})", condition.bits()),
            );
        }
        Ok(condition) => {
            line("hardware", &format!("register {}", condition.bits()));
            for fault in condition.faults() {
                println!("    - {}", fault.describe());
            }
        }
        Err(e) => line("hardware", &format!("unavailable: {e}")),
    }

    println!("\nStatus");
    show(
        "alarm",
        device.alarm_condition().map(|a| {
            if a.is_clear() {
                "clear".to_owned()
            } else {
                format!("{} (clear it at the receiver)", a.named_bits().join(", "))
            }
        }),
    );
    show(
        "operation",
        device.operation_condition().map(|op| {
            format!(
                "register {}: {}",
                op.bits(),
                flags(&[
                    ("locked", op.locked()),
                    ("position hold", op.position_hold()),
                    ("reference valid", op.reference_valid()),
                    ("log almost full", op.log_almost_full()),
                ])
            )
        }),
    );
    show(
        "powerup",
        device.powerup_condition().map(|up| {
            format!(
                "register {}: {}",
                up.bits(),
                flags(&[
                    ("first satellite tracked", up.first_satellite_tracked()),
                    ("oven warm", up.oven_warm()),
                    ("date and time valid", up.date_time_valid()),
                ])
            )
        }),
    );

    println!("\nHoldover");
    match device.holdover_duration() {
        Ok(holdover) => {
            let state = if holdover.active {
                "in holdover"
            } else {
                "not in holdover"
            };
            line("state", &format!("{state}, last {}", holdover.elapsed));
        }
        Err(e) => line("state", &format!("unavailable: {e}")),
    }
    show(
        "predicted 24 h",
        device
            .holdover_predicted()
            .map(|v| absent_or(v, |v| v.to_string())),
    );
    show(
        "present error",
        device
            .holdover_present()
            .map(|v| absent_or(v, |v| v.to_string())),
    );

    println!("\nGPS");
    match (device.tracking_count(), device.visible_count()) {
        (Ok(tracked), Ok(visible)) => {
            line(
                "satellites",
                &format!("{tracked} tracked of {visible} predicted"),
            );
        }
        (Ok(tracked), Err(_)) => line("satellites", &format!("{tracked} tracked")),
        (Err(e), _) => line("satellites", &format!("unavailable: {e}")),
    }

    show("UTC", device.time().map(|t| t.to_string()));

    // Checked against the host clock because firmware predating the
    // 2019 GPS week rollover reports a date exactly 1024 weeks in the
    // past.  Its time of day and outputs are sound.
    let today = Zoned::now().date();
    match device.date(today) {
        Ok(date) => line("date", &date_text(date)),
        Err(e) => line("date", &format!("unavailable: {e}")),
    }

    println!("\nLog");
    show("entries", device.log_count().map(|n| n.to_string()));
    Ok(())
}

/// A receiver's date, corrected first: almost every receiver of this
/// vintage is behind by whole epochs, so this is the ordinary case
/// rather than a fault, and its time of day and outputs are unaffected
/// either way.  The raw date stays beside it because the correction is
/// computed against the host clock and is only as good as that clock
/// is.
fn date_text(date: ReceiverDate) -> String {
    match date.rollover() {
        Some(slip) => format!(
            "{}  ({slip} applied, {} days; reported {})",
            date.corrected(),
            slip.days(),
            date.raw()
        ),
        None => date.raw().to_string(),
    }
}

/// The conditions that are true, named, or "none".
///
/// A register printed as a number says nothing without the manual, and
/// the bits that are clear are not worth a line each.
fn flags(set: &[(&str, bool)]) -> String {
    let named: Vec<&str> = set
        .iter()
        .filter(|(_, on)| *on)
        .map(|(name, _)| *name)
        .collect();
    if named.is_empty() {
        "none".to_owned()
    } else {
        named.join(", ")
    }
}

/// The column the values line up in.
const LABEL_WIDTH: usize = 18;

/// Print one label and its text, with the label padded to the column.
///
/// Every line in `diagnose` goes through here, so the width is written
/// once.  An empty label continues the previous one.
fn line(label: &str, text: &str) {
    println!("  {label:<LABEL_WIDTH$} {text}");
}

/// Print one field, or why it could not be read.
///
/// A value that could not be read is said to be unavailable, never
/// shown as a default: this is the tool someone points at a receiver
/// they suspect.
///
/// Infallible, though it takes a `Result`: the failure it reports is
/// the receiver's, not its own.  It used to return `Result<()>` that
/// could never be `Err`, which put a `?` on ten call sites for nothing.
fn show(label: &str, value: smartclock::error::Result<String>) {
    match value {
        Ok(text) => line(label, &text),
        Err(e) => line(label, &format!("unavailable: {e}")),
    }
}

/// Render an optional reading, saying so when the receiver declined it.
///
/// Takes the value rather than returning a closure, which spared every
/// call site a type annotation it only needed to satisfy inference.
fn absent_or<T>(value: Option<T>, render: impl FnOnce(T) -> String) -> String {
    match value {
        Some(value) => render(value),
        None => "not applicable in this state".to_owned(),
    }
}

fn probe<T: Transport>(session: &mut Session<T>, dialect: Option<&str>) -> Result<()> {
    let dialect = match dialect {
        Some("hp58503") => Dialect::Hp58503,
        Some("z3801") => Dialect::Z3801,
        Some(other) => anyhow::bail!("unknown dialect {other:?}"),
        None => {
            let reply = session
                .query("*IDN?")
                .context("asking the receiver its model")?;
            let identity = parse::identity(reply.one_line("identity")?)?;
            let dialect = dialect_for(&identity.model);
            println!("{identity}: probing the {dialect:?} command tree\n");
            dialect
        }
    };

    let mut answered = 0usize;
    let mut refused = 0usize;
    let mut failed = 0usize;

    for spec in dialect.specs() {
        if spec.class != Class::Query {
            continue;
        }
        match session.query(spec.scpi) {
            Ok(reply) => {
                answered += 1;
                println!("ok       {:<52} {}", spec.scpi, reply.lines.join(" | "));
            }
            Err(Error::Device { code, message }) => {
                refused += 1;
                println!("refused  {:<52} {code}: {message}", spec.scpi);
            }
            Err(e) => {
                failed += 1;
                println!("FAILED   {:<52} {e}", spec.scpi);
                // A timeout leaves the receiver still sending.  sync()
                // drains before provoking a prompt, so the next command
                // is not read one reply behind.
                session.sync().context("resynchronizing after a failure")?;
            }
        }
        std::io::stdout().flush().ok();
    }

    println!("\n{answered} answered, {refused} refused, {failed} failed");
    Ok(())
}

/// Serve a subcommand from a running daemon rather than from the port.
///
/// Only the subcommands that make sense through another process: a
/// query is one exchange, and diagnose is a rendering of state the
/// daemon already holds.  `probe` and `sweep` send hundreds of commands
/// and belong on a port of their own, with the daemon stopped.
fn through_daemon(socket: &Path, command: &Command) -> Result<()> {
    let mut daemon = Daemon::connect(socket).with_context(|| {
        format!(
            "connecting to {}; is smartclockd running, and are you in its group?",
            socket.display()
        )
    })?;
    match command {
        Command::Query { commands } => {
            for one in commands {
                let reply = daemon.query(one)?;
                for line in reply {
                    println!("{line}");
                }
            }
            Ok(())
        }
        Command::Diagnose => diagnose_daemon(&mut daemon),
        Command::Note { text, at } => {
            filed("noted", &daemon.note(&text.join(" "), *at)?);
            Ok(())
        }
        Command::Fact { key, value, since } => {
            filed("recorded", &daemon.fact(key, &value.join(" "), *since)?);
            Ok(())
        }
        Command::Commands => {
            print!("{}", smartclock::matrix::markdown());
            Ok(())
        }
        Command::Sensors { sensor_socket } => sensors(sensor_socket),
        Command::Probe { .. } | Command::Sweep { .. } => anyhow::bail!(
            "probe and sweep send hundreds of commands and need the port to themselves; \
             stop smartclockd and use --device"
        ),
        Command::Flash(_) => anyhow::bail!(
            "flash talks to the installer on the port, not to the daemon; \
             stop smartclockd for that port and use --device"
        ),
        Command::ReadMemory { .. } | Command::ReadFlash { .. } | Command::ReadEeprom { .. } => {
            anyhow::bail!(
                "reading memory talks to the pForth console, not to the daemon; \
                 stop smartclockd for that port and use --device"
            )
        }
    }
}

/// Print every sensor and its latest reading, one line each: the value
/// while it is current, and otherwise what is known of why not.
fn sensors(socket: &Path) -> Result<()> {
    let latest = Daemon::connect(socket)
        .and_then(|mut service| service.sensor_readings())
        .with_context(|| {
            format!(
                "asking {}; is smartclock-sensord running, and are you in its group?",
                socket.display()
            )
        })?;
    let now = Timestamp::now();
    for reading in &latest.readings {
        let name = format!("{} {}", reading.name, reading.quantity);
        let age = |at: Timestamp| now.duration_since(at).as_secs_f64().round();
        match (reading.current(latest.every_s, now), reading.at) {
            (Some(value), Some(at)) => {
                println!("{name}: {value} {}, {} s ago", reading.unit, age(at));
            }
            (_, at) => {
                let last = match (reading.value, at) {
                    (Some(value), Some(at)) => {
                        format!("last {value} {}, {} s ago", reading.unit, age(at))
                    }
                    _ => "never read".to_owned(),
                };
                let why = reading
                    .error
                    .as_deref()
                    .map_or(String::new(), |e| format!("; {e}"));
                println!("{name}: no current reading ({last}{why})");
            }
        }
    }
    Ok(())
}

/// Say where a note or fact went.
fn filed(done: &str, went: &Filed) {
    if went.written {
        println!("{done} for {}", went.receiver);
    } else {
        println!(
            "queued for {}; the daemon is busy and writes it shortly",
            went.receiver
        );
    }
}

/// Print a daemon's reading in the shape `diagnose` prints.
///
/// The ages come from the reading's own per-tier state, so a field the
/// daemon has not refreshed lately says so rather than looking current.
fn render(info: &serde_json::Value, r: &Reading, facts: &Result<Vec<Fact>>) {
    let text = |key: &str| {
        info.get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
    };
    println!("{}", text("identity"));
    line("dialect", text("dialect"));
    line("source", &format!("daemon, reading taken {}", r.at));

    println!("\nFacts");
    match facts {
        Ok(facts) if facts.is_empty() => line("", "none recorded"),
        Ok(facts) => {
            for fact in facts {
                line(
                    &fact.key,
                    &format!("{}  (since {})", fact.value, fact.since),
                );
            }
        }
        Err(e) => line("", &format!("unavailable: {e:#}")),
    }

    println!("\nLock");
    show_opt("mode", r.mode.as_ref().map(ToString::to_string));
    show_opt(
        "waiting to recover",
        r.holdover_waiting.as_ref().map(ToString::to_string),
    );
    show_opt("TFOM", r.tfom.map(|v| v.to_string()));
    show_opt("FFOM", r.ffom.map(|v| v.to_string()));
    show_opt(
        "1 PPS TI",
        r.time_interval_ns.map(|v| format!("{v:+.1} ns")),
    );

    println!("\nOscillator");
    show_opt("EFC", r.efc.map(|v| v.to_string()));
    show_opt(
        "internal temp",
        r.temperature_c.map(|v| format!("{v:.2} C")),
    );
    show_opt("oven current", r.oven_current.map(|v| format!("{v:.1}")));
    show_opt(
        "current coeff",
        r.oven_tempco
            .map(|v| format!("{v:.2} EFC counts per unit of oven current")),
    );
    show_opt("EFC raw", r.efc_raw.map(|v| v.to_string()));
    show_opt(
        "hardware",
        r.hardware.map(|condition| {
            if condition.is_healthy() {
                format!("no faults (register {})", condition.bits())
            } else {
                format!("register {}", condition.bits())
            }
        }),
    );

    println!("\nStatus");
    show_opt(
        "alarm",
        r.alarm.map(|a| {
            if a.is_clear() {
                "clear".to_owned()
            } else {
                format!("{} (clear it at the receiver)", a.named_bits().join(", "))
            }
        }),
    );
    show_opt(
        "operation",
        r.locked.map(|_| {
            flags(&[
                ("locked", r.locked == Some(true)),
                ("position hold", r.position_hold == Some(true)),
                ("reference valid", r.reference_valid == Some(true)),
                ("log almost full", r.log_almost_full == Some(true)),
            ])
        }),
    );
    show_opt(
        "powerup",
        r.oven_warm.map(|_| {
            flags(&[
                (
                    "first satellite tracked",
                    r.first_satellite_tracked == Some(true),
                ),
                ("oven warm", r.oven_warm == Some(true)),
                ("date and time valid", r.date_time_valid == Some(true)),
            ])
        }),
    );

    println!("\nHoldover");
    show_opt(
        "state",
        r.holdover_active.map(|active| {
            let state = if active {
                "in holdover"
            } else {
                "not in holdover"
            };
            format!("{state}, last {:.0} s", r.holdover_seconds.unwrap_or(0.0))
        }),
    );
    // Through Seconds so the socket path scales the same way the
    // direct path does: "432.0 us", not "0.0003212 s".
    show_opt(
        "predicted 24 h",
        r.holdover_predicted_s.map(|v| Seconds::new(v).to_string()),
    );
    show_opt(
        "present error",
        r.holdover_present_s.map(|v| Seconds::new(v).to_string()),
    );

    println!("\nGPS");
    show_opt("UTC", r.time.map(|t| t.to_string()));
    match r.screen.as_ref() {
        Some(screen) => {
            show_opt(
                "satellites",
                screen
                    .tracking
                    .map(|n| format!("{n} tracked of {} in view", screen.satellites.len())),
            );
            if screen.satellites_suspect {
                line("", "the table disagrees with these counts");
            }
        }
        None => line("satellites", "unavailable: no status screen yet"),
    }
    match r.date {
        Some(date) => line("date", &date_text(date)),
        None => line("date", "unavailable"),
    }

    println!("\nLog");
    show_opt("entries", r.log_count.map(|n| n.to_string()));

    println!("\nFreshness");
    for (tier, state) in [
        ("fast", &r.polled.fast),
        ("medium", &r.polled.medium),
        ("slow", &r.polled.slow),
    ] {
        let text = match (&state.at, &state.error) {
            (Some(at), None) => format!("last read {at}"),
            (Some(at), Some(e)) => format!("last read {at}, then: {e}"),
            (None, Some(e)) => format!("never read: {e}"),
            (None, None) => "never read".to_owned(),
        };
        line(tier, &text);
    }
}

/// Print a field the daemon may not have, saying so when it has not.
fn show_opt(label: &str, value: Option<String>) {
    match value {
        Some(text) => line(label, &text),
        None => line(label, "unavailable"),
    }
}

/// Report the receiver's health from the daemon's own last reading.
///
/// Sends nothing to the receiver.  The daemon polls all of this anyway,
/// so asking it again would cost link time and tell no one anything
/// new; the age of each tier is printed instead, so a value that has
/// not been refreshed says so.
fn diagnose_daemon(daemon: &mut Daemon) -> Result<()> {
    let info = daemon.info()?;
    let reading = daemon.latest()?;
    render(&info, &reading, &facts(&info));
    Ok(())
}

/// The current facts about the daemon's receiver, read from its log.
///
/// The daemon names its log; reading it needs the same group a monitor
/// needs.
fn facts(info: &serde_json::Value) -> Result<Vec<Fact>> {
    let path = info
        .get("database")
        .and_then(serde_json::Value::as_str)
        .filter(|p| !p.is_empty())
        .context("the daemon did not say where its log is")?;
    let identity = info
        .get("identity")
        .and_then(serde_json::Value::as_str)
        .context("the daemon did not say what it is attached to")?;
    let serial = parse::identity(identity)?.serial;
    let log = Log::open(Path::new(path)).with_context(|| format!("reading {path}"))?;
    let Some(receiver) = log.receivers()?.into_iter().find(|r| r.serial == serial) else {
        return Ok(Vec::new());
    };
    Ok(log.facts(receiver.id)?)
}

#[cfg(test)]
mod tests {
    use super::seconds;
    use super::transcript;
    use std::time::Duration;

    #[test]
    fn a_transcript_is_a_new_file() {
        let path = std::env::temp_dir().join(format!(
            "smartclock-cli-transcript-{}.jsonl",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        transcript(&path).expect("a new file is created");
        assert!(transcript(&path).is_err(), "an existing file was reopened");
        std::fs::remove_file(&path).expect("removing the test file");
    }

    #[test]
    fn a_timeout_is_a_positive_finite_number_of_seconds() {
        assert_eq!(seconds("2.5"), Ok(Duration::from_millis(2500)));
        for bad in ["-1", "0", "nan", "inf", "soon"] {
            assert!(seconds(bad).is_err(), "{bad} was accepted");
        }
    }
}
