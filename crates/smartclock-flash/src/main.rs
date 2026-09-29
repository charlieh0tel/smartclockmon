//! Direct-port SmartClock installer. Protocol: 097-58503-13, 5-115 and
//! appendix C; Model-specific bounds and checks: docs/firmware.md.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::ensure;
use clap::Parser;
use smartclock::attach::answering;
use smartclock::console;
use smartclock::console::LANGUAGE_SETTLE;
use smartclock::console::Progress;
use smartclock::parse;
use smartclock::parse::Identity;
use smartclock::session::Config;
use smartclock::session::Session;
use smartclock::transport;
use smartclock::transport::Transport;
use smartclock::transport::serial::Settings;
use smartclock::transport::tee::TeeTransport;
use smartclock::types::BaudRate;
use smartclock::types::Framing;
use thiserror::Error;

mod firmware;

use firmware::Firmware;
use firmware::IMAGE_SIZE;
use firmware::PROFILES;
use firmware::RECORD_SIZE;

const SERIAL_READ_TIMEOUT: Duration = Duration::from_millis(250);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const PROGRESS_INTERVAL_BYTES: usize = 0x2000;
const COMPATIBILITY_NOTICE: &str =
    "Compatibility uses a model/layout allowlist; installer/primary pairing is not verified.";

/// Flashing is opt-in; without a device only the file is inspected.
#[derive(Debug, Parser)]
#[command(about = "Check or flash SmartClock firmware. Stop the port's daemon first.", version = smartclock::VERSION)]
struct Cli {
    /// Full 512 KiB, address-zero binary dump (not an individual chip or S-record file).
    image: PathBuf,
    /// Serial port to open directly, or `tcp://host:port`. No daemon or
    /// automatic port discovery.
    #[arg(long)]
    device: Option<String>,
    /// Rate tried first; the others the receivers use are tried when it
    /// gets no answer.
    #[arg(long, default_value_t = 19200)]
    baud: u32,
    /// Framing tried first, `7O1` or `8N1`; as `--baud`.
    #[arg(long, default_value = "7O1")]
    framing: Framing,
    /// Erase and program after all compatibility checks pass.
    #[arg(long, requires_all = ["device", "serial", "capture"])]
    write: bool,
    /// Expected receiver serial number; required for writing.
    #[arg(long)]
    serial: Option<String>,
    /// New JSONL transcript file; required for writing, never overwritten.
    #[arg(long)]
    capture: Option<PathBuf>,
    /// Skip reading the whole flash back through the debug console
    /// after writing.
    #[arg(long)]
    no_readback: bool,
}

/// The state machine uses the same exchanges in tests and on the wire.
trait Link {
    fn command(&mut self, command: &str) -> Result<String>;
    fn settle(&mut self) -> Result<()>;
}

/// The two audited interpreters and their corresponding firmware IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Primary,
    Installer,
}

/// Compatibility failures must never be bypassed by a write option.
#[derive(Debug, Error)]
enum CompatibilityError {
    #[error("image is for {expected}, receiver is {actual}")]
    Model {
        expected: &'static str,
        actual: String,
    },
    #[error("receiver serial mismatch or missing serial: {0}")]
    Serial(String),
    #[error("unsupported running firmware or installer: {0}")]
    Revision(String),
    #[error("incompatible flash layout")]
    Layout,
    #[error("identity and language disagree: {identity}, {language}")]
    Language { identity: String, language: String },
}

impl<T: Transport> Link for Session<T> {
    fn command(&mut self, command: &str) -> Result<String> {
        let reply = self.query(command)?;
        Ok(reply.lines.join("\n"))
    }

    fn settle(&mut self) -> Result<()> {
        // Language changes can acknowledge before the old interpreter exits.
        std::thread::sleep(LANGUAGE_SETTLE);
        self.sync()?;
        Ok(())
    }
}

fn receiver(
    link: &mut impl Link,
    firmware: &Firmware,
    serial: Option<&str>,
) -> Result<(Identity, Mode)> {
    let id = parse::identity(&link.command("*IDN?")?)?;
    if !matches!(id.manufacturer.as_str(), "HEWLETT-PACKARD" | "SYMMETRICOM")
        || id.model != firmware.profile.model
    {
        return Err(CompatibilityError::Model {
            expected: firmware.profile.model,
            actual: id.to_string(),
        }
        .into());
    }
    if id.serial.is_empty() || serial.is_some_and(|serial| serial != id.serial) {
        return Err(CompatibilityError::Serial(id.to_string()).into());
    }
    let (revision, suffix) = id
        .firmware
        .rsplit_once('-')
        .ok_or_else(|| CompatibilityError::Revision(id.to_string()))?;
    if suffix.len() != 1 || !suffix.bytes().all(|byte| byte.is_ascii_uppercase()) {
        return Err(CompatibilityError::Revision(id.to_string()).into());
    }
    let profile = PROFILES
        .iter()
        .find(|profile| {
            profile.model == id.model
                && (profile.revision == revision || profile.installer == revision)
        })
        .ok_or_else(|| CompatibilityError::Revision(id.to_string()))?;
    if profile.layout != firmware.profile.layout {
        return Err(CompatibilityError::Layout.into());
    }
    let language = link.command(":SYSTem:LANGuage?")?;
    let mode = match language.trim().trim_matches('"') {
        "PRIMARY" if revision == profile.revision => Mode::Primary,
        "INSTALL" if revision == profile.installer => Mode::Installer,
        _ => {
            return Err(CompatibilityError::Language {
                identity: id.to_string(),
                language,
            }
            .into());
        }
    };
    Ok((id, mode))
}

fn no_error(link: &mut impl Link) -> Result<()> {
    let text = link.command(":SYSTem:ERRor?")?;
    let error = parse::error_entry(&text)?;
    ensure!(error.code == 0, "receiver error: {text}");
    Ok(())
}

fn flash(
    link: &mut impl Link,
    firmware: &Firmware,
    write: bool,
    serial: Option<&str>,
) -> Result<()> {
    ensure!(
        !write || serial.is_some(),
        "writing requires an expected serial number"
    );
    let (before, mode) = receiver(link, firmware, serial)?;
    println!("Receiver: {before}; mode {mode:?}");
    no_error(link).context(
        "preflight error check failed; review queued errors with :SYSTem:ERRor? and \
         clear remaining errors with *CLS before retrying; the flasher does not send *CLS",
    )?;
    if !write {
        println!("{COMPATIBILITY_NOTICE}");
        println!("Compatible. Check only: no language change, erase or programming.");
        return Ok(());
    }
    if mode == Mode::Primary {
        link.command(":SYSTem:LANGuage \"INSTALL\"")?;
        link.settle()?;
    }
    let suffix = before
        .firmware
        .rsplit_once('-')
        .expect("validated revision")
        .1;
    // Boot flash survives upgrades, so the installer need not be the one
    // bundled with the running primary. receiver checks its model/layout
    // allowlist membership; here we check the transition and suffix.
    let (installer, mode) = receiver(link, firmware, Some(&before.serial))?;
    ensure!(
        mode == Mode::Installer
            && installer
                .firmware
                .rsplit_once('-')
                .map(|(_, suffix)| suffix)
                == Some(suffix),
        "unexpected installer: {installer}"
    );
    no_error(link)?;
    println!("{COMPATIBILITY_NOTICE}");
    program(link, firmware).context(
        "flash did not complete; keep the daemon stopped. No automatic retry or reboot was attempted. \
         Rerun with the same image and serial from the installer to erase and start again")?;
    // TEST? 1 reads cached boot flags; it cannot verify a new download.
    // PRIMARY reruns the boot lane checks before jumping into the image.
    link.command(":SYSTem:LANGuage \"PRIMARY\"")?;
    link.settle()?;
    let (after, mode) = receiver(link, firmware, Some(&before.serial))?;
    let (revision, after_suffix) = after.firmware.rsplit_once('-').expect("validated revision");
    ensure!(
        mode == Mode::Primary && revision == firmware.profile.revision,
        "primary did not boot after flashing: {after}; leave daemon stopped"
    );
    if after_suffix != suffix {
        println!(
            "Booted {}; suffix changed from {suffix} to {after_suffix}.",
            after.firmware
        );
    }
    println!("Verified primary boot: {after}");
    Ok(())
}

fn program(link: &mut impl Link, firmware: &Firmware) -> Result<()> {
    let start = firmware.profile.layout.primary_start();
    eprintln!("Erasing {start:#07x}..0x7ffff; preserving boot flash and EEPROM.");
    link.command(":DIAGnostic:ERASe")?;
    ensure!(
        link.command(":DIAGnostic:ERASe?")?.trim().parse::<i32>()? == 1,
        "erase verification failed; no records sent"
    );
    no_error(link)?;
    for (address, record) in firmware.records() {
        link.command(&format!(":DIAGnostic:DOWNload \"{record}\""))
            .with_context(|| format!("programming record at {address:#07x}"))?;
        // A prompt acknowledges each record. Error queue checks also catch
        // faults when the receiver's prompt is not configured to show them.
        no_error(link).with_context(|| format!("checking record at {address:#07x}"))?;
        if (address - start).is_multiple_of(PROGRESS_INTERVAL_BYTES) {
            eprintln!(
                "Programmed through {:#07x} ({:.0}%)",
                address + RECORD_SIZE - 1,
                100.0 * (address + RECORD_SIZE - start) as f64 / (IMAGE_SIZE - start) as f64
            );
        }
    }
    no_error(link)?;
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let firmware = Firmware::validate(
        std::fs::read(&cli.image).with_context(|| format!("reading {}", cli.image.display()))?,
    )?;
    println!(
        "Image: {} {}; SHA-256 {}; boot checksums valid",
        firmware.profile.model, firmware.profile.revision, firmware.profile.sha256
    );
    let Some(path) = cli.device else {
        return Ok(());
    };
    let baud = BaudRate::new(cli.baud).context("unsupported baud rate")?;
    // Create the transcript before opening the hardware; an existing file
    // or unwritable destination must never fail after erase.
    let capture = cli
        .capture
        .map(|path| {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .with_context(|| format!("creating {}", path.display()))
        })
        .transpose()?;
    eprintln!("Opening only {path}; its daemon must be stopped.");
    let config = Config {
        timeout: COMMAND_TIMEOUT,
        ..Config::default()
    };
    let settings = Settings {
        path,
        baud,
        framing: cli.framing,
        read_timeout: SERIAL_READ_TIMEOUT,
    };
    let port = open(&settings, &config, capture)?;
    let mut session = Session::new(port, config.clone());
    session.sync()?;
    flash(&mut session, &firmware, cli.write, cli.serial.as_deref())?;
    if !cli.write || cli.no_readback {
        return Ok(());
    }
    let profile = firmware.profile;
    if console::exit_for(profile.model, profile.revision).is_none() {
        println!(
            "No debug-console exit is known for {} {}; not reading the flash back.",
            profile.model, profile.revision
        );
        return Ok(());
    }
    readback(session.into_transport(), &firmware, config)
}

/// Open the receiver at the line settings it answers at -- `settings`,
/// or the ones the probe finds -- recording the exchange to `capture`
/// when given.  The probe runs before anything that could change the
/// unit, and reads the errors its garbled bytes leave off the queue.
fn open(
    settings: &Settings,
    config: &Config,
    capture: Option<std::fs::File>,
) -> Result<Box<dyn Transport>> {
    let (settings, report) = answering(settings, config)
        .with_context(|| format!("no receiver answered on {}", settings.path))?;
    if let Some(report) = report {
        eprintln!("{report}");
    }
    let port = transport::open(&settings)?;
    Ok(match capture {
        Some(file) => Box::new(TeeTransport::new(port, file)),
        None => port,
    })
}

/// Read the whole flash back through the debug console and compare it
/// with the image, protected boot region included, then return the port
/// to SCPI through the installer.
fn readback(port: Box<dyn Transport>, firmware: &Firmware, config: Config) -> Result<()> {
    eprintln!("Reading the flash back through the debug console.");
    let (summary, identity) = console::read_and_return(
        port,
        0,
        u32::try_from(IMAGE_SIZE).expect("the image fits the address space"),
        &mut std::io::sink(),
        Some(firmware.bytes()),
        |progress| {
            if let Progress::Read { done, elapsed, .. } = progress {
                eprintln!("Read back {done:#07x} in {} s", elapsed.as_secs());
            }
        },
        config,
    )
    .context("readback did not complete; the flash was already verified to boot")?;
    ensure!(
        summary.differing.is_empty(),
        "flash differs from the image in the 1 KiB blocks at {:#x?}; {identity}",
        summary.differing
    );
    println!(
        "Read back all {IMAGE_SIZE:#x} bytes in {} s: identical to the image. {identity}",
        summary.took.as_secs()
    );
    Ok(())
}

#[cfg(test)]
mod tests;
