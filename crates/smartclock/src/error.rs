//! Typed errors for the library.

use std::time::Duration;

/// Anything that can go wrong talking to a receiver.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Opening or configuring the serial port failed.
    #[error("serial port: {0}")]
    Serial(#[from] serialport::Error),

    /// A read or write failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// No prompt arrived before the deadline.  `seen` is whatever did
    /// arrive, which is usually the most useful thing for diagnosis.
    #[error("no prompt within {waited:?}; received {seen:?}")]
    Timeout {
        /// How long the read waited.
        waited: Duration,
        /// Bytes received before giving up, decoded lossily.
        seen: String,
    },

    /// The receiver answered with an entry from its error queue.
    #[error("receiver error {code}: {message}")]
    Device {
        /// SCPI error code; negative codes are standard, positive are
        /// device specific.
        code: i32,
        /// The quoted description the receiver returned.
        message: String,
    },

    /// The receiver signalled an error but its error queue was empty,
    /// which means the two have drifted out of step.
    #[error("receiver signalled error {prompt:?} but its error queue was empty")]
    UnexplainedError {
        /// The error prompt that was seen.
        prompt: String,
    },

    /// A reply could not be parsed as the command's documented format.
    #[error("cannot parse {reply:?} as {expected}")]
    Parse {
        /// What the receiver sent.
        reply: String,
        /// The response format that was expected.
        expected: &'static str,
    },

    /// The device task was asked for something after it stopped.
    #[error("the device task has stopped: {0}")]
    TaskStopped(&'static str),

    /// This dialect has no spelling for the operation.  Held as a typed
    /// error so a caller learns of it without a command reaching the
    /// receiver.
    #[error("{dialect} has no command for {operation}")]
    Unsupported {
        /// The command tree in use.
        dialect: &'static str,
        /// The operation that has no spelling.
        operation: &'static str,
    },

    /// The transcript being replayed ran out, or diverged from what the
    /// caller sent.
    #[error("replay: {0}")]
    Replay(String),
}

impl Error {
    /// Whether reopening the port could plausibly fix this.
    ///
    /// The distinction decides when the daemon gives up on a link.  A
    /// timeout or an I/O error means the port; a parse error or a
    /// refusal from the receiver means the command or the reply, and
    /// reconnecting repeats it forever.  Counting every error toward
    /// the reconnect threshold put the daemon in a five-second loop,
    /// logging nothing, on a receiver whose only fault was one reply
    /// the parser did not expect.
    pub fn is_link_failure(&self) -> bool {
        matches!(
            self,
            Self::Io(_) | Self::Serial(_) | Self::Timeout { .. } | Self::UnexplainedError { .. }
        )
    }
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;
