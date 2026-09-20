//! The client protocol: newline-delimited JSON over a local socket.
//!
//! Debuggable with `socat`, and not tied to Rust on either end.  The
//! stream is multiplexed -- snapshots arrive unsolicited while replies
//! interleave -- so every request carries an id the reply echoes.

use serde::Deserialize;
use serde::Serialize;
use smartclock::snapshot::Snapshot;
use smartclock::wire::Reading;

/// Bumped when the message shapes change.  Daemon and clients are
/// upgraded separately, so both ends check it.
pub(crate) const VERSION: u32 = 1;

/// A message from a client.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Request {
    /// Protocol version the client speaks.
    pub(crate) v: u32,
    /// Correlates the reply.  Echoed back verbatim.
    pub id: String,
    /// What to do.
    pub op: Op,
}

/// What a client is asking for.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(crate) enum Op {
    /// Send a read-only command and return its reply.
    Query {
        /// The SCPI string.  Rejected unless the command table marks it
        /// a query: control lands in phase 6.
        scpi: String,
    },
    /// Return the most recent snapshot without waiting.
    Latest,
    /// Report what the daemon is attached to.
    Info,
}

/// A message from the daemon.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub(crate) enum Message {
    /// An unsolicited reading.
    Event {
        /// Protocol version.
        v: u32,
        /// Always `snapshot`.
        event: &'static str,
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
    pub(crate) fn event(snapshot: &Snapshot) -> Self {
        Self::Event {
            v: VERSION,
            event: "snapshot",
            snapshot: Box::new(Reading::from(snapshot)),
        }
    }

    /// A successful reply.
    pub(crate) fn ok(id: String, value: serde_json::Value) -> Self {
        Self::Reply {
            v: VERSION,
            id,
            ok: Some(value),
            err: None,
        }
    }

    /// A failed reply.
    pub(crate) fn err(id: String, why: impl std::fmt::Display) -> Self {
        Self::Reply {
            v: VERSION,
            id,
            ok: None,
            err: Some(why.to_string()),
        }
    }
}
