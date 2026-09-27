//! Opening a receiver at line settings other than the configured ones.
//!
//! The simulator has no line settings, so each test gives it only the
//! settings the "unit" is really at; every other attempt gets a port
//! that hears nothing, which is what a receiver at the wrong framing
//! looks like from here.  What the wrong framing leaves behind -- the
//! garbled bytes the receiver queued as errors -- is seeded into the
//! simulator's queue.

use std::io::Read;
use std::io::Write;
use std::time::Duration;

use smartclock::attach::attach_with;
use smartclock::error::Error;
use smartclock::session::Config;
use smartclock::transport::Transport;
use smartclock::transport::serial::Settings;
use smartclock::types::BaudRate;
use smartclock::types::Framing;
use smartclock_sim::receiver::Receiver;
use smartclock_sim::transport::SimTransport;

/// A port with nothing on the other end that understands it.
struct Deaf;

impl Read for Deaf {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Ok(0)
    }
}

impl Write for Deaf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Transport for Deaf {
    fn describe(&self) -> String {
        "a port hearing nothing".to_owned()
    }
}

/// Quick to give up, since every wrong setting costs one timeout.
fn quick() -> Config {
    Config {
        timeout: Duration::from_millis(300),
        idle: Duration::from_millis(20),
        ..Config::default()
    }
}

fn configured(baud: BaudRate, framing: Framing) -> Settings {
    Settings {
        path: "/dev/ttyUSB-test".to_owned(),
        baud,
        framing,
        ..Settings::default()
    }
}

/// An opener for a unit really at `at`, holding `receiver`.
fn unit_at(
    at: (BaudRate, Framing),
    receiver: Receiver,
) -> impl FnMut(&Settings) -> smartclock::error::Result<Box<dyn Transport + Send>> {
    let mut receiver = Some(receiver);
    move |tried: &Settings| {
        Ok(if (tried.baud, tried.framing) == at {
            Box::new(SimTransport::new(
                receiver.take().expect("opened once at the right settings"),
            )) as Box<dyn Transport + Send>
        } else {
            Box::new(Deaf)
        })
    }
}

#[test]
fn a_unit_at_other_settings_is_found_and_its_probe_errors_drained() {
    let mut receiver = Receiver::default();
    // What garbled bytes at the wrong framing leave behind.
    receiver.queue_error(-102, "Syntax error");
    receiver.queue_error(-113, "Undefined header");
    let at = (BaudRate::B19200, Framing::SevenOdd);
    let attached = attach_with(
        &configured(BaudRate::B19200, Framing::EightNone),
        &quick(),
        unit_at(at, receiver),
    )
    .expect("found at 7O1");
    assert_eq!((attached.baud, attached.framing), at);
    assert!(attached.probed);
    assert_eq!(attached.discarded, 2);
    assert_eq!(attached.device.identity().model, "58503A");
}

#[test]
fn a_unit_at_its_configured_settings_is_opened_as_before() {
    let mut receiver = Receiver::default();
    // Already there, and the daemon's to journal: not drained.
    receiver.queue_error(-313, "Calibration memory lost");
    let at = (BaudRate::B9600, Framing::EightNone);
    let attached = attach_with(&configured(at.0, at.1), &quick(), unit_at(at, receiver))
        .expect("opened at the configured settings");
    assert!(!attached.probed);
    assert_eq!(attached.discarded, 0);
}

#[test]
fn a_unit_answering_at_no_settings_reports_the_configured_attempt() {
    let settings = configured(BaudRate::B19200, Framing::EightNone);
    let mut opened = Vec::new();
    let result = attach_with(&settings, &quick(), |tried: &Settings| {
        opened.push((tried.baud, tried.framing));
        Ok(Box::new(Deaf) as Box<dyn Transport + Send>)
    });
    let error = result.err().expect("no settings answered");
    assert!(matches!(error, Error::Timeout { .. }), "{error:?}");
    assert_eq!(opened[0], (settings.baud, settings.framing));
    assert_eq!(opened.len(), 4, "{opened:?}");
}

#[test]
fn a_port_that_will_not_open_is_not_probed() {
    let mut opened = 0;
    let result = attach_with(
        &configured(BaudRate::B19200, Framing::EightNone),
        &quick(),
        |_: &Settings| -> smartclock::error::Result<Box<dyn Transport + Send>> {
            opened += 1;
            Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no such device",
            )))
        },
    );
    assert!(result.is_err());
    assert_eq!(opened, 1);
}
