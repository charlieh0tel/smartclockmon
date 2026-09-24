//! Replays a recorded transcript in place of a receiver.

use std::collections::VecDeque;
use std::io::Read;
use std::io::Write;

use crate::error::Error;
use crate::error::Result;
use crate::transport::Transport;
use crate::transport::transcript::Direction;
use crate::transport::transcript::Record;

/// A receiver substituted by a transcript.
///
/// Writes are checked against what was recorded, so a test that changes
/// the commands it sends fails loudly rather than silently reading
/// someone else's replies.
#[derive(Debug)]
pub struct ReplayTransport {
    records: VecDeque<Record>,
    /// Receiver bytes not yet handed to the caller.
    pending: VecDeque<u8>,
    /// Host bytes written but not yet matched against a Tx record.
    written: Vec<u8>,
    strict: bool,
    /// Whether dropping it with the transcript unfinished is a failure;
    /// see the `Drop` impl.
    must_finish: bool,
}

impl ReplayTransport {
    /// Load a transcript from JSONL text.
    pub fn from_jsonl(text: &str) -> Result<Self> {
        let mut records = VecDeque::new();
        for (n, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let record: Record = serde_json::from_str(line)
                .map_err(|e| Error::Replay(format!("line {}: {e}", n + 1)))?;
            records.push_back(record);
        }
        Ok(Self {
            records,
            pending: VecDeque::new(),
            written: Vec::new(),
            strict: true,
            must_finish: true,
        })
    }

    /// Stop checking that writes match the transcript.  Useful when
    /// replaying a capture taken with a different command sequence.
    pub fn relaxed(mut self) -> Self {
        self.strict = false;
        self
    }

    /// Allow the replay to end partway, for a test whose point is that
    /// it stops early -- a command the transcript does not contain.
    pub fn unfinished(mut self) -> Self {
        self.must_finish = false;
        self
    }

    /// Whether every record has been consumed and nothing extra was
    /// written.
    ///
    /// Unmatched written bytes count: without them a test could send a
    /// command absent from the capture, have the write silently
    /// accepted because no Tx record was left to compare it against,
    /// and still be told the transcript was fully replayed.
    pub fn exhausted(&self) -> bool {
        self.records.is_empty() && self.pending.is_empty() && self.written.is_empty()
    }

    /// Move receiver bytes into `pending`, skipping any Tx records that
    /// the caller has already matched.
    fn fill(&mut self) {
        while let Some(front) = self.records.front() {
            if front.dir != Direction::Rx {
                break;
            }
            let record = self.records.pop_front().expect("front checked");
            self.pending.extend(record.data.as_bytes());
        }
    }
}

impl Read for ReplayTransport {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.fill();
        let n = buf.len().min(self.pending.len());
        for slot in buf.iter_mut().take(n) {
            *slot = self.pending.pop_front().expect("length checked");
        }
        Ok(n)
    }
}

impl Write for ReplayTransport {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.written.extend_from_slice(buf);

        // Consume Tx records as the caller's writes cover them.  In
        // strict mode a write with no Tx record left to match is a
        // command the capture does not contain, which is exactly what
        // strict mode exists to catch.
        if self.strict
            && !self.written.is_empty()
            && !self.records.iter().any(|r| r.dir == Direction::Tx)
        {
            return Err(std::io::Error::other(format!(
                "replay: sent {:?}, which the transcript does not contain",
                String::from_utf8_lossy(&self.written)
            )));
        }
        while let Some(front) = self.records.front() {
            if front.dir != Direction::Tx {
                break;
            }
            let expected = front.data.as_bytes();
            if self.written.len() < expected.len() {
                break;
            }
            let (head, rest) = self.written.split_at(expected.len());
            if self.strict && head != expected {
                return Err(std::io::Error::other(format!(
                    "replay diverged: sent {:?}, transcript has {:?}",
                    String::from_utf8_lossy(head),
                    front.data
                )));
            }
            self.written = rest.to_vec();
            self.records.pop_front();
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A strict replay that ends with records unread or bytes unmatched
/// did not replay what it claims to, and says so.
///
/// Without this a test could stop partway through its transcript, or
/// send one command too many at the end, and pass: nothing else ever
/// looked at whether the conversation it scripted actually happened.
/// Skipped while already panicking, since a second panic would abort
/// the test runner and hide the first.
impl Drop for ReplayTransport {
    fn drop(&mut self) {
        if self.strict && self.must_finish && !std::thread::panicking() && !self.exhausted() {
            panic!(
                "replay ended with {} records unread and {:?} sent unmatched",
                self.records.len(),
                String::from_utf8_lossy(&self.written)
            );
        }
    }
}

impl Transport for ReplayTransport {
    fn describe(&self) -> String {
        "replayed transcript".to_owned()
    }
}
