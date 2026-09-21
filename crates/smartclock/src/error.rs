//! Typed errors for the library.

use std::time::Duration;

use crate::command::CommandId;

/// Whether a device error code means the receiver declined on state.
///
/// -221 is a settings conflict and -230 is data stale.  Both mean the
/// header parsed and the value simply does not exist right now --
/// present holdover error while locked, survey progress while in hold
/// -- so a caller should read them as absent rather than as failure.
/// A function rather than three places testing the two numbers.
pub(crate) fn is_state_refusal(code: i32) -> bool {
    const STATE_REFUSALS: [i32; 2] = [-221, -230];
    STATE_REFUSALS.contains(&code)
}

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
    #[error("{dialect} has no command for {operation:?}")]
    Unsupported {
        /// The command tree in use.
        dialect: &'static str,
        /// The operation that has no spelling.
        operation: CommandId,
    },

    /// The transcript being replayed ran out, or diverged from what the
    /// caller sent.
    /// A daemon refused a request, or answered with something this
    /// client could not use.  Carries the daemon's own words.
    #[error("the daemon said: {0}")]
    Daemon(String),

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
    ///
    /// `UnexplainedError` is the awkward one and is counted: the
    /// receiver raised an error prompt and then had nothing in its
    /// queue to explain it, which means the session and the receiver
    /// are out of step rather than that the command was wrong, and
    /// reopening does fix that.
    pub fn is_link_failure(&self) -> bool {
        matches!(
            self,
            Self::Io(_) | Self::Serial(_) | Self::Timeout { .. } | Self::UnexplainedError { .. }
        )
    }
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;
