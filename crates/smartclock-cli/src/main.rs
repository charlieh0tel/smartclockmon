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
use smartclock::error::Error;
use smartclock::parse;
use smartclock::rollover::ReceiverDate;
use smartclock::session::Config;
use smartclock::session::Session;
use smartclock::transport::Transport;
use smartclock::transport::serial::SerialTransport;
use smartclock::transport::serial::Settings;
use smartclock::transport::tee::TeeTransport;
use smartclock::types::BaudRate;
use smartclock::types::HardwareCondition;
use smartclock::types::SmartClockMode;

#[derive(Parser)]
#[command(about, version)]
struct Cli {
    /// Serial device.  Prefer a /dev/serial/by-id/... path, which
    /// survives USB re-enumeration.
    #[arg(long, default_value = "/dev/ttyUSB0", global = true)]
    device: String,

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
    let settings = Settings {
        path: cli.device.clone(),
        baud,
        read_timeout: Duration::from_millis(250),
    };
    let config = Config {
        timeout: Duration::from_secs_f64(cli.timeout),
        ..Config::default()
    };
    let port = SerialTransport::open(&settings)
        .with_context(|| format!("opening {} at {} baud", cli.device, cli.baud))?;

    // Capture wraps the port, so a recording covers the sync exchange
    // too, not just the commands the subcommand issues.
    match &cli.capture {
        Some(path) => {
            let sink = File::create(path)
                .with_context(|| format!("creating transcript {}", path.display()))?;
            let mut session = Session::new(TeeTransport::new(port, sink), config);
            let result = run(&mut session, &cli.command);
            eprintln!("transcript written to {}", path.display());
            result
        }
        None => run(&mut Session::new(port, config), &cli.command),
    }
}

fn run<T: Transport>(session: &mut Session<T>, command: &Command) -> Result<()> {
    session
        .sync()
        .with_context(|| format!("no prompt from {}", session.describe()))?;

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
        Command::Diagnose => diagnose(session),
        Command::Sweep { from } => sweep(session, from),
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

/// Ask one query and hand back the single line it produces.
///
/// A receiver that declines on state, -221 or -230, is answering
/// truthfully about a value that does not exist right now, so that is
/// reported as absent rather than as a failure.
fn ask<T: Transport>(session: &mut Session<T>, scpi: &str) -> Result<Option<String>> {
    match session.query(scpi) {
        Ok(reply) => Ok(reply.lines.first().cloned()),
        Err(Error::Device { code, .. }) if code == -221 || code == -230 => Ok(None),
        Err(e) => Err(e).with_context(|| format!("sending {scpi}")),
    }
}

/// Print what the receiver says about its oscillator and its lock.
fn diagnose<T: Transport>(session: &mut Session<T>) -> Result<()> {
    if let Some(line) = ask(session, "*IDN?")? {
        let id = parse::identity(&line)?;
        println!(
            "{} {}  serial {}  firmware {}",
            id.manufacturer, id.model, id.serial, id.firmware
        );
    }

    println!("\nLock");
    if let Some(line) = ask(session, ":SYNChronization:STATe?")? {
        let mode = SmartClockMode::parse(&line)
            .map_or_else(|| format!("unrecognised ({line})"), |m| format!("{m:?}"));
        println!("  mode                {mode}");
    }
    if let Some(line) = ask(session, ":SYNChronization:HOLDover:WAITing?")? {
        println!("  waiting to recover  {line}");
    }
    for (label, scpi) in [
        ("TFOM", ":SYNChronization:TFOMerit?"),
        ("FFOM", ":SYNChronization:FFOMerit?"),
    ] {
        if let Some(line) = ask(session, scpi)? {
            println!("  {label:<18}  {}", parse::int(&line)?);
        }
    }
    if let Some(line) = ask(session, ":SYNChronization:TINTerval?")? {
        println!("  1 PPS interval      {}", parse::seconds(&line)?);
    }

    println!("\nOscillator");
    if let Some(line) = ask(session, ":DIAGnostic:ROSCillator:EFControl:RELative?")? {
        let efc = parse::efc(&line)?;
        // The signed percentage is less informative than how much of
        // the tuning range is gone: an aged OCXO fails by walking to a
        // rail, whichever one.
        println!(
            "  EFC                 {efc}  ({:.0}% of tuning range used)",
            efc.range_used() * 100.0
        );
    }
    if let Some(line) = ask(session, ":STATus:OPERation:HARDware:CONDition?")? {
        let bits = u16::try_from(parse::int(&line)?).unwrap_or(0);
        let condition = HardwareCondition::from_bits(bits);
        if condition.is_healthy() {
            println!("  hardware            no faults (register {bits})");
        } else {
            println!("  hardware            register {bits}");
            for fault in condition.faults() {
                println!("    - {}", fault.describe());
            }
        }
    }

    println!("\nHoldover");
    if let Some(line) = ask(session, ":SYNChronization:HOLDover:DURation?")? {
        let holdover = parse::holdover_duration(&line)?;
        let state = if holdover.active {
            "in holdover"
        } else {
            "not in holdover"
        };
        println!(
            "  state               {state}, last duration {}",
            holdover.elapsed
        );
    }
    if let Some(line) = ask(session, ":SYNChronization:HOLDover:TUNCertainty:PREDicted?")? {
        println!(
            "  predicted 24 h      {}",
            parse::holdover_duration(&line)?.elapsed
        );
    }
    match ask(session, ":SYNChronization:HOLDover:TUNCertainty:PRESent?")? {
        Some(line) => println!("  present error       {}", parse::seconds(&line)?),
        None => println!("  present error       not applicable outside holdover"),
    }

    println!("\nGPS");
    if let Some(line) = ask(session, ":GPS:SATellite:TRACking:COUNt?")? {
        let visible = ask(session, ":GPS:SATellite:VISible:PREDicted:COUNt?")?
            .map(|v| parse::int(&v))
            .transpose()?;
        match visible {
            Some(visible) => println!(
                "  satellites          {} tracked of {visible} predicted",
                parse::int(&line)?
            ),
            None => println!("  satellites          {} tracked", parse::int(&line)?),
        }
    }

    // The date is checked against the host clock because this firmware
    // predates the 2019 GPS week rollover and reports a date exactly
    // 1024 weeks in the past.  Its time of day and outputs are sound.
    if let Some(line) = ask(session, ":PTIMe:DATE?")? {
        let raw = parse::ymd(&line)?;
        let today = Zoned::now().date();
        let seen = ReceiverDate::checked(raw, today);
        match seen.rollover() {
            Some(slip) => {
                println!("  date                {raw}  WRONG");
                println!(
                    "                      {} GPS week rollover(s), {} days behind; actual date is {}",
                    slip.epochs,
                    slip.days(),
                    seen.corrected()
                );
                println!("                      time of day, 1 PPS and 10 MHz are unaffected");
            }
            None => println!("  date                {raw}"),
        }
    }

    println!("\nLog");
    if let Some(line) = ask(session, ":DIAGnostic:LOG:COUNt?")? {
        println!("  entries             {}", parse::int(&line)?);
    }
    if let Some(line) = ask(session, ":DIAGnostic:LOG:READ?")? {
        println!("  most recent         {}", parse::string(&line)?);
    }
    Ok(())
}

/// Send every query in a dialect and report what came back.
///
/// Only queries: probing must never change the receiver's state, and
/// the table already records which commands would.
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
