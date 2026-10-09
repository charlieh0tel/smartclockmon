//! What other threads ask the log thread to write: commands that were
//! not scheduled polls, and notes and facts from a person.
//!
//! Writing goes through a channel rather than a shared connection: the
//! log has one writer, the thread that owns it, and a client thread
//! blocking on a database write would hold up its own reply.

use std::sync::mpsc::RecvTimeoutError;
use std::sync::mpsc::Sender;
use std::sync::mpsc::channel;
use std::time::Duration;

use smartclock::command::Class;

/// One command as it happened.
#[derive(Debug)]
pub(crate) struct Entry {
    /// When it completed.  Taken here, not when the row is written: the
    /// log thread can be a journal pass behind, and a row stamped then
    /// could land after the snapshots showing the command's effect.
    pub(crate) at: jiff::Timestamp,
    /// The `*IDN?` of the receiver it was sent to, as the daemon had it
    /// when the command was classified.
    pub(crate) receiver: String,
    /// What was sent.
    pub(crate) scpi: String,
    /// How the table classified it.
    pub(crate) class: String,
    /// What came back.
    pub(crate) outcome: String,
}

/// How long a client's note or fact waits for the log thread to say it
/// was written.  Short of the client's own deadline, so the client
/// hears an answer rather than timing out.
const WRITTEN_WITHIN: Duration = Duration::from_secs(15);

/// Free text for the log, as a client sent it.
#[derive(Debug)]
pub(crate) struct Note {
    /// When it happened.
    pub(crate) at: jiff::Timestamp,
    /// The `*IDN?` of the receiver attached when it was sent.
    pub(crate) receiver: String,
    /// What it says.
    pub(crate) text: String,
}

/// A fact about the unit, as a client sent it.
#[derive(Debug)]
pub(crate) struct Fact {
    /// When the value became true.
    pub(crate) since: jiff::Timestamp,
    /// The `*IDN?` of the receiver attached when it was sent.
    pub(crate) receiver: String,
    /// What it is about, such as `ocxo.serial`.
    pub(crate) key: String,
    /// Its value.
    pub(crate) value: String,
}

/// A change to a note already written, as a client sent it.
#[derive(Debug)]
pub(crate) struct NoteChange {
    /// Which note, by its id in the log.
    pub(crate) id: i64,
    /// The `*IDN?` of the receiver attached when it was sent.
    pub(crate) receiver: String,
    /// The text the client read, if it read one.
    pub(crate) was: Option<String>,
    /// Its new text and, if given, its new time; `None` to delete it.
    pub(crate) now: Option<(String, Option<jiff::Timestamp>)>,
}

/// Whether a write happened, and if not, why.
type Written = Sender<Result<(), String>>;

/// What became of a note or fact the log thread accepted.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Filed {
    /// It is in the log.
    Written,
    /// The log thread was busy past [`WRITTEN_WITHIN`], in a journal
    /// pass; it will be written, and a failure then is logged.  Said
    /// rather than reported as a failure, since a client retrying a
    /// write that was going to happen anyway would write it twice.
    Queued,
}

/// What another thread asks the log thread, the log's one writer, to do.
#[derive(Debug)]
pub(crate) enum LogRequest {
    /// Record a command.
    Audit(Entry),
    /// Write a note and say whether it was written.
    Note(Note, Written),
    /// Write a fact, and a note saying so, and say whether they were
    /// written.
    Fact(Fact, Written),
    /// Change or delete a note, and say whether it was done.
    NoteChange(NoteChange, Written),
}

/// A handle on the log thread's inbox.
#[derive(Debug, Clone)]
pub(crate) struct LogInbox {
    requests: Sender<LogRequest>,
}

impl LogInbox {
    /// Record into `sink`.
    pub(crate) fn new(sink: Sender<LogRequest>) -> Self {
        Self { requests: sink }
    }

    /// Note a command.  Failing to record must not fail the command:
    /// the audit trail is a record of what happened, not a gate on it.
    pub(crate) fn audit(&self, scpi: &str, class: Class, outcome: &str, receiver: &str) {
        let _ = self.requests.send(LogRequest::Audit(Entry {
            at: jiff::Timestamp::now(),
            receiver: receiver.to_owned(),
            scpi: scpi.to_owned(),
            class: format!("{class:?}").to_lowercase(),
            outcome: outcome.to_owned(),
        }));
    }

    /// Write a note, waiting up to [`WRITTEN_WITHIN`] to hear it was.
    pub(crate) fn note(&self, note: Note) -> Result<Filed, String> {
        self.write(|written| LogRequest::Note(note, written))
    }

    /// Write a fact, waiting up to [`WRITTEN_WITHIN`] to hear it was.
    pub(crate) fn fact(&self, fact: Fact) -> Result<Filed, String> {
        self.write(|written| LogRequest::Fact(fact, written))
    }

    /// Change or delete a note, waiting up to [`WRITTEN_WITHIN`] to
    /// hear it was.
    pub(crate) fn change_note(&self, change: NoteChange) -> Result<Filed, String> {
        self.write(|written| LogRequest::NoteChange(change, written))
    }

    fn write(&self, request: impl FnOnce(Written) -> LogRequest) -> Result<Filed, String> {
        let (written, outcome) = channel();
        self.requests
            .send(request(written))
            .map_err(|_| "the log thread has stopped".to_owned())?;
        match outcome.recv_timeout(WRITTEN_WITHIN) {
            Ok(Ok(())) => Ok(Filed::Written),
            Ok(Err(why)) => Err(why),
            Err(RecvTimeoutError::Timeout) => Ok(Filed::Queued),
            Err(RecvTimeoutError::Disconnected) => {
                Err("the log thread stopped before writing it".to_owned())
            }
        }
    }
}
