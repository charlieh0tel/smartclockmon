//! The client protocol: newline-delimited JSON over a local socket.
//!
//! In the library rather than in the daemon because more than one
//! program speaks it: the daemon serves it, and the monitor and the
//! command line tool both talk to it.
//!
//! Debuggable with `socat`, and not tied to Rust on either end.  The
//! stream is multiplexed -- snapshots arrive unsolicited while replies
//! interleave -- so every request carries an id the reply echoes.
//!
//! The envelope -- a versioned request with an id, and a reply that
//! echoes it -- is shared by every service that speaks this framing;
//! each names its own requests and their version through [`Protocol`].

use serde::Deserialize;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::snapshot::Snapshot;
use crate::wire::Reading;

/// Bumped when the message shapes change.
///
/// Checked by the daemon, which is enough: every exchange begins with a
/// client's request, so a mismatch in either direction is refused
/// before anything is acted on, and the client is told which version
/// the daemon speaks rather than failing to parse a reply.
pub const VERSION: u32 = 1;

/// The requests one service answers, and the version of their shapes.
pub trait Protocol: Serialize + DeserializeOwned {
    /// Bumped when the shapes of these requests or their replies change.
    const VERSION: u32;
}

impl Protocol for Op {
    const VERSION: u32 = VERSION;
}

/// A message from a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound(deserialize = "O: DeserializeOwned"))]
pub struct Request<O = Op> {
    /// Protocol version the client speaks.
    pub v: u32,
    /// Correlates the reply.  Echoed back verbatim.
    pub id: String,
    /// What to do.
    pub op: O,
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
    /// Read one status screen and return it, as `{"screen": ...}`.
    ///
    /// No tier polls the screen: it costs 1.5 s, four fast passes, and
    /// per-satellite elevation, azimuth and signal strength are all it
    /// still answers alone.  A client showing the status view asks for
    /// one while it is being looked at, and so pays for what it shows.
    Status,
    /// Report what the daemon is attached to.
    Info,
    /// Write a note to the attached receiver's log.  Nothing is sent to
    /// the receiver.  Answers `{"receiver": ..., "written": ...}`: the
    /// unit it was filed under, and whether it is in the log yet or
    /// queued behind the daemon's own work, to be written shortly.
    Note {
        /// What it says.
        text: String,
        /// When it happened; now if absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<jiff::Timestamp>,
    },
    /// Record a fact about the attached receiver, such as
    /// `ocxo.serial`, and a note saying so.  Nothing is sent to the
    /// receiver.  Answers as [`Op::Note`] does.
    Fact {
        /// What it is about.
        key: String,
        /// Its value.
        value: String,
        /// When it became true; now if absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since: Option<jiff::Timestamp>,
    },
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
        Self::ok_in(VERSION, id, value)
    }

    /// A failed reply.
    pub fn err(id: String, why: impl std::fmt::Display) -> Self {
        Self::err_in(VERSION, id, why)
    }

    /// A successful reply in protocol version `v`.
    pub fn ok_in(v: u32, id: String, value: serde_json::Value) -> Self {
        Self::Reply {
            v,
            id,
            ok: Some(value),
            err: None,
        }
    }

    /// A failed reply in protocol version `v`.
    pub fn err_in(v: u32, id: String, why: impl std::fmt::Display) -> Self {
        Self::Reply {
            v,
            id,
            ok: None,
            err: Some(why.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Op;
    use super::Protocol;
    use super::Request;
    use serde::Deserialize;
    use serde::Serialize;

    /// Another service's requests, in the same envelope.
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "lowercase")]
    enum Other {
        Ping,
    }

    impl Protocol for Other {
        const VERSION: u32 = 7;
    }

    #[test]
    fn another_services_requests_travel_in_the_same_envelope() {
        let request = Request {
            v: Other::VERSION,
            id: "1".to_owned(),
            op: Other::Ping,
        };
        let line = serde_json::to_string(&request).expect("encode");
        assert_eq!(line, r#"{"v":7,"id":"1","op":{"kind":"ping"}}"#);
        let back: Request<Other> = serde_json::from_str(&line).expect("decode");
        assert_eq!(back.op, Other::Ping);
        // And not as the receiver daemon's.
        assert!(serde_json::from_str::<Request<Op>>(&line).is_err());
    }
}
