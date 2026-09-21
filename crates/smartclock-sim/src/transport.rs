//! The simulated receiver behind a [`Transport`].
//!
//! Framing is the part worth simulating.  The receiver echoes what it
//! is sent, ends every exchange with `scpi > `, and switches to
//! `E-nnn > ` after a bad command; those are what the session layer
//! exists to handle, and a simulator that skipped them would test
//! nothing that matters.

use std::collections::VecDeque;
use std::io::Read;
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;

use smartclock::transport::Transport;

use crate::receiver::Receiver;

/// Longest command the simulator will accumulate before giving up on
/// it.  Generous next to any real command.
const MAX_PARTIAL: usize = 4096;

/// The prompt, spaced as the receiver spaces it.
///
/// Note the space before the angle bracket.  The manuals print
/// `scpi>`; the wire carries `scpi > `, and getting that wrong is what
/// stopped the first attempt at talking to a real one.
const PROMPT: &str = "scpi > ";

/// A simulated receiver on the other end of a transport.
#[derive(Debug, Clone)]
pub struct SimTransport {
    receiver: Arc<Mutex<Receiver>>,
    /// Bytes the caller has yet to read.
    pending: Arc<Mutex<VecDeque<u8>>>,
    /// The command being typed, up to its terminator.
    partial: Arc<Mutex<String>>,
    /// Whether the last command was refused, which changes the prompt.
    failed: Arc<Mutex<Option<i32>>>,
}

impl Default for SimTransport {
    fn default() -> Self {
        Self::new(Receiver::default())
    }
}

impl SimTransport {
    /// Wrap a receiver.
    pub fn new(receiver: Receiver) -> Self {
        let transport = Self {
            receiver: Arc::new(Mutex::new(receiver)),
            pending: Arc::new(Mutex::new(VecDeque::new())),
            partial: Arc::new(Mutex::new(String::new())),
            failed: Arc::new(Mutex::new(None)),
        };
        // A real receiver is already sitting at a prompt when something
        // connects to it.
        transport.emit(PROMPT);
        transport
    }

    /// The receiver, for a test that wants to change or inspect it.
    pub fn receiver(&self) -> &Arc<Mutex<Receiver>> {
        &self.receiver
    }

    fn emit(&self, text: &str) {
        self.pending
            .lock()
            .expect("pending mutex")
            .extend(text.as_bytes());
    }

    /// Handle one complete command line.
    fn run(&self, command: &str) {
        let answer = self
            .receiver
            .lock()
            .expect("receiver mutex")
            .respond(command);
        for line in &answer.lines {
            self.emit(line);
            self.emit("\r\n");
        }
        if answer.accepted {
            *self.failed.lock().expect("failed mutex") = None;
            self.emit(PROMPT);
        } else {
            // The receiver reports the code in the prompt and keeps the
            // detail in its error queue until someone reads it.
            *self.failed.lock().expect("failed mutex") = Some(-113);
            self.emit("E-113 > ");
        }
    }
}

impl Read for SimTransport {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut pending = self.pending.lock().expect("pending mutex");
        let n = buf.len().min(pending.len());
        for slot in buf.iter_mut().take(n) {
            *slot = pending.pop_front().expect("length checked");
        }
        Ok(n)
    }
}

impl Write for SimTransport {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Echo, as the receiver does, a character at a time as they
        // arrive.
        self.emit(&String::from_utf8_lossy(buf));

        let mut partial = self.partial.lock().expect("partial mutex");
        // A client that never sends a newline would otherwise grow this
        // without limit, and each write would rescan more of it.  The
        // receiver has a finite input buffer too; overrunning it
        // discards the line rather than the simulator's memory.
        if partial.len() + buf.len() > MAX_PARTIAL {
            partial.clear();
            self.emit("\r\nE-363 > ");
            return Ok(buf.len());
        }
        partial.push_str(&String::from_utf8_lossy(buf));
        // A command ends at a newline; carriage returns are part of the
        // terminator, not of the command.
        while let Some(at) = partial.find('\n') {
            let line: String = partial.drain(..=at).collect();
            let command = line.trim_end_matches(['\r', '\n']).to_owned();
            drop(partial);
            self.run(&command);
            partial = self.partial.lock().expect("partial mutex");
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Transport for SimTransport {
    fn describe(&self) -> String {
        "a simulated receiver".to_owned()
    }
}
