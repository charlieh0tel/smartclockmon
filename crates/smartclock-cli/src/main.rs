//! One-shot queries, transcript capture, and command table probing.
//!
//! Talks to the receiver directly.  `smartclockd` holds the serial port
//! open once it exists, so it must be stopped before using this.

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context as _;
use anyhow::Result;
use clap::Parser;
use clap::Subcommand;
use jiff::Zoned;
use smartclock::command::Class;
use smartclock::command::Dialect;
use smartclock::device::Device;
use smartclock::error::Error;
use smartclock::session::Config;
use smartclock::session::Session;
use smartclock::transport;
use smartclock::transport::Transport;
use smartclock::transport::serial::Settings;
use smartclock::transport::tee::TeeTransport;
use smartclock::types::BaudRate;

#[derive(Parser)]
#[command(about, version)]
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
    /// necessarily the 9600 factory default.  Checked against the four
    /// rates the receiver accepts: an unsupported rate does not fail on
    /// open, it produces garbage that looks like a dead receiver.
    #[arg(long, default_value_t = 19200, global = true)]
    baud: u32,

    /// Seconds to wait for a prompt.
    #[arg(long, default_value_t = 5.0, global = true)]
    timeout: f64,

    /// Record the whole exchange to this JSONL transcript.
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
        /// Which command tree to probe.
        #[arg(long, default_value = "hp58503")]
        dialect: String,
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Reading the table needs no receiver, so this runs before the port
    // is opened rather than failing for want of hardware.
    if matches!(cli.command, Command::Commands) {
        print!("{}", smartclock::matrix::markdown());
        return Ok(());
    }

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
    let device = cli.device.clone().context(
        "--device is required and has no default; SMARTCLOCK_DEVICE works too. \
         Try: ls -l /dev/serial/by-id/",
    )?;
    let settings = Settings {
        path: device.clone(),
        baud,
        read_timeout: Duration::from_millis(250),
    };
    let config = Config {
        timeout: Duration::from_secs_f64(cli.timeout),
        ..Config::default()
    };
    let port = transport::open(&settings)
        .with_context(|| format!("opening {device} at {} baud", cli.baud))?;

    // Capture wraps the port, so a recording covers the sync exchange
    // too, not just the commands the subcommand issues.
    match &cli.capture {
        Some(path) => {
            let sink = File::create(path)
                .with_context(|| format!("creating transcript {}", path.display()))?;
            let session = Session::new(TeeTransport::new(port, sink), config);
            let result = run(session, &cli.command);
            eprintln!("transcript written to {}", path.display());
            result
        }
        None => run(Session::new(port, config), &cli.command),
    }
}

fn run<T: Transport>(mut session: Session<T>, command: &Command) -> Result<()> {
    session
        .sync()
        .with_context(|| format!("no prompt from {}", session.describe()))?;

    // diagnose builds a Device, which takes the session, so it is
    // handled before the borrowing arms below.
    if matches!(command, Command::Diagnose) {
        return diagnose(session);
    }
    let session = &mut session;

    match command {
        Command::Query { commands } => {
            for one in commands {
                let reply = session
                    .query(one)
                    .with_context(|| format!("sending {one}"))?;
                for line in &reply.lines {
                    println!("{line}");
                }
            }
            Ok(())
        }
        Command::Probe { dialect } => probe(session, dialect),
        Command::Sweep { from } => sweep(session, from),
        Command::Diagnose => unreachable!("handled above, since it takes the session"),
        // Handled before the port is opened.
        Command::Commands => Ok(()),
    }
}

/// Send each candidate and report what the receiver makes of it.
fn sweep<T: Transport>(session: &mut Session<T>, from: &PathBuf) -> Result<()> {
    let text =
        std::fs::read_to_string(from).with_context(|| format!("reading {}", from.display()))?;
    let candidates: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();

    let mut known = Vec::new();
    let (mut unknown, mut refused, mut failed) = (0usize, 0usize, 0usize);
    for scpi in &candidates {
        match session.query(scpi) {
            Ok(reply) => {
                let value = reply.lines.join(" | ");
                println!("FOUND    {scpi:<52} {value}");
                known.push((*scpi, value));
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
                session.sync().context("resynchronising after a failure")?;
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
        "1 PPS interval",
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
        "temperature",
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

    // Checked against the host clock because firmware predating the
    // 2019 GPS week rollover reports a date exactly 1024 weeks in the
    // past.  Its time of day and outputs are sound.
    let today = Zoned::now().date();
    match device.date(today) {
        Ok(date) => match date.rollover() {
            Some(slip) => {
                line("date", &format!("{}  WRONG", date.raw()));
                line(
                    "",
                    &format!(
                        "{} after {} GPS week rollover(s), {} days",
                        date.corrected(),
                        slip.epochs,
                        slip.days()
                    ),
                );
                line("", "time of day, 1 PPS and 10 MHz are unaffected");
            }
            None => line("date", &date.raw().to_string()),
        },
        Err(e) => line("date", &format!("unavailable: {e}")),
    }

    println!("\nLog");
    show("entries", device.log_count().map(|n| n.to_string()));
    Ok(())
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

fn probe<T: Transport>(session: &mut Session<T>, dialect: &str) -> Result<()> {
    let dialect = match dialect {
        "hp58503" => Dialect::Hp58503,
        "z3801" => Dialect::Z3801,
        other => anyhow::bail!("unknown dialect {other:?}"),
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
                session.sync().context("resynchronising after a failure")?;
            }
        }
        std::io::stdout().flush().ok();
    }

    println!("\n{answered} answered, {refused} refused, {failed} failed");
    Ok(())
}
