//! Recording commands that were not scheduled polls.
//!
//! Writing goes through a channel rather than a shared connection: the
//! log has one writer, the thread that owns it, and a client thread
//! blocking on a database write would hold up its own reply.

use std::sync::mpsc::Sender;

use smartclock::command::Class;

/// One command as it happened.
#[derive(Debug)]
pub(crate) struct Entry {
    /// What was sent.
    pub(crate) scpi: String,
    /// How the table classified it.
    pub(crate) class: String,
    /// What came back.
    pub(crate) outcome: String,
}

/// A handle for recording commands.
#[derive(Debug, Clone)]
pub(crate) struct Audit {
    entries: Sender<Entry>,
}

impl Audit {
    /// Record into `sink`.
    pub(crate) fn new(sink: Sender<Entry>) -> Self {
        Self { entries: sink }
    }

    /// Note a command.  Failing to record must not fail the command:
    /// the audit trail is a record of what happened, not a gate on it.
    pub(crate) fn record(&self, scpi: &str, class: Class, outcome: &str) {
        let _ = self.entries.send(Entry {
            scpi: scpi.to_owned(),
            class: format!("{class:?}").to_lowercase(),
            outcome: outcome.to_owned(),
        });
    }
}
