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
use smartclock::command::Class;
use smartclock::command::Dialect;
use smartclock::error::Error;
use smartclock::session::Config;
use smartclock::session::Session;
use smartclock::transport::Transport;
use smartclock::transport::serial::SerialTransport;
use smartclock::transport::serial::Settings;
use smartclock::transport::tee::TeeTransport;

#[derive(Parser)]
#[command(about, version)]
struct Cli {
    /// Serial device.  Prefer a /dev/serial/by-id/... path, which
    /// survives USB re-enumeration.
    #[arg(long, default_value = "/dev/ttyUSB0", global = true)]
    device: String,

    /// Bits per second.  The receiver stores this setting, so it is not
    /// necessarily the 9600 factory default.
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let settings = Settings {
        path: cli.device.clone(),
        baud: cli.baud,
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
    }
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
