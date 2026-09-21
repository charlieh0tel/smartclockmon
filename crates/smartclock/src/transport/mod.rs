//! Byte transports to a receiver.
//!
//! Keeping this a trait lets the session layer run against a serial
//! port, a recorded transcript, or later a simulator, so most of the
//! stack is testable without hardware.

pub mod replay;
pub mod serial;
pub mod tcp;
pub mod tee;
pub mod transcript;

use std::io::Read;
use std::io::Write;

/// A bidirectional byte stream to a receiver.
///
/// Reads are expected to block up to some implementation-defined
/// timeout and then return `Ok(0)`, rather than erroring, so the
/// session layer can distinguish "nothing yet" from a real failure.
pub trait Transport: Read + Write {
    /// A short description used in error messages and logs, such as the
    /// device path.
    fn describe(&self) -> String;
}

/// So a caller that picks a transport at run time can hold one.
///
/// The daemon does: a device path may name a serial port or a receiver
/// on the network, and which is known only once the path is read.
impl<T: Transport + ?Sized> Transport for Box<T> {
    fn describe(&self) -> String {
        (**self).describe()
    }
}

/// The prefix that means a receiver on the network rather than a local
/// serial port.
const NETWORK_PREFIX: &str = "tcp://";

/// Open whatever `settings.path` names.
///
/// A `tcp://host:port` path reaches a receiver over the network, which
/// covers both a serial adapter behind ser2net and the simulator; the
/// simulator listens on TCP rather than offering a pseudo-terminal,
/// since a PTY would confine it to Unix.  Anything else is a serial
/// device, and `baud` applies only to that case.
///
/// Here rather than in each binary so that every tool accepts the same
/// paths.  It lived in the daemon alone, which is why the CLI -- the
/// tool most likely to be pointed at an unfamiliar receiver, and the
/// only one that can capture a transcript -- could not be pointed at
/// the simulator at all.
pub fn open(settings: &serial::Settings) -> crate::error::Result<Box<dyn Transport + Send>> {
    match settings.path.strip_prefix(NETWORK_PREFIX) {
        Some(address) => Ok(Box::new(tcp::TcpTransport::connect(
            address,
            settings.read_timeout,
        )?)),
        None => Ok(Box::new(serial::SerialTransport::open(settings)?)),
    }
}
