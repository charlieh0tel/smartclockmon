//! Byte transports to a receiver.
//!
//! Keeping this a trait lets the session layer run against a serial
//! port, a recorded transcript, or later a simulator, so most of the
//! stack is testable without hardware.

pub mod replay;
pub mod serial;
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
