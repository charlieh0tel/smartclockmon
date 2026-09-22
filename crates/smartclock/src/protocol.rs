//! The client protocol: newline-delimited JSON over a local socket.
//!
//! In the library rather than in the daemon because more than one
//! program speaks it: the daemon serves it, and the monitor and the
//! command line tool both talk to it.
//!
//! Debuggable with `socat`, and not tied to Rust on either end.  The
//! stream is multiplexed -- snapshots arrive unsolicited while replies
//! interleave -- so every request carries an id the reply echoes.

use serde::Deserialize;
use serde::Serialize;

use crate::snapshot::Snapshot;
use crate::wire::Reading;

/// Bumped when the message shapes change.
///
/// Checked by the daemon, which is enough: every exchange begins with a
/// client's request, so a mismatch in either direction is refused
/// before anything is acted on, and the client is told which version
/// the daemon speaks rather than failing to parse a reply.
pub const VERSION: u32 = 1;

/// A message from a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// Protocol version the client speaks.
    pub v: u32,
    /// Correlates the reply.  Echoed back verbatim.
    pub id: String,
    /// What to do.
    pub op: Op,
}

/// What a client is asking for.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Op {
    /// Send a read-only command and return its reply.
    Query {
        /// The SCPI string.  What it is allowed to be depends on the
        /// flags the daemon was started with, not on the name of this
        /// variant.
        scpi: String,
    },
    /// Return the most recent snapshot without waiting.
    Latest,
    /// Report what the daemon is attached to.
    Info,
}

/// A message from the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    /// An unsolicited reading.
    Event {
        /// Protocol version.
        v: u32,
        /// Always `snapshot`.
        event: String,
        /// The reading.
        snapshot: Box<Reading>,
    },
    /// A reply to a request.
    Reply {
        /// Protocol version.
        v: u32,
        /// The request's id.
        id: String,
        /// The answer, when it worked.
        #[serde(skip_serializing_if = "Option::is_none")]
        ok: Option<serde_json::Value>,
        /// Why it did not, when it did not.
        #[serde(skip_serializing_if = "Option::is_none")]
        err: Option<String>,
    },
}

impl Message {
    /// Wrap a reading for broadcast.
    pub fn event(snapshot: &Snapshot) -> Self {
        Self::Event {
            v: VERSION,
            event: "snapshot".to_owned(),
            snapshot: Box::new(Reading::from(snapshot)),
        }
    }

    /// A successful reply.
    pub fn ok(id: String, value: serde_json::Value) -> Self {
        Self::Reply {
            v: VERSION,
            id,
            ok: Some(value),
            err: None,
        }
    }

    /// A failed reply.
    pub fn err(id: String, why: impl std::fmt::Display) -> Self {
        Self::Reply {
            v: VERSION,
            id,
            ok: None,
            err: Some(why.to_string()),
        }
    }
}
