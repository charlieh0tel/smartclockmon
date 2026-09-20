//! A transport decorator that records everything to a transcript.
//!
//! Captures double as replay fixtures, so recording against the real
//! receiver is how the test corpus gets built.

use std::io::BufWriter;
use std::io::Read;
use std::io::Write;
use std::time::Instant;

use crate::error::Result;
use crate::transport::Transport;
use crate::transport::transcript::Direction;
use crate::transport::transcript::Record;

/// Wraps a transport and writes every read and write to a JSONL sink.
#[derive(Debug)]
pub struct TeeTransport<T: Transport, W: Write> {
    inner: T,
    sink: BufWriter<W>,
    started: Instant,
}

impl<T: Transport, W: Write> TeeTransport<T, W> {
    /// Begin recording `inner` into `sink`.
    pub fn new(inner: T, sink: W) -> Self {
        Self {
            inner,
            sink: BufWriter::new(sink),
            started: Instant::now(),
        }
    }

    /// Flush any buffered transcript lines.
    pub fn finish(mut self) -> Result<T> {
        self.sink.flush()?;
        Ok(self.inner)
    }

    fn record(&mut self, dir: Direction, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let record = Record::new(self.started.elapsed().as_secs_f64(), dir, bytes);
        // A transcript is diagnostic, so a failure to write one must not
        // take down the session that is producing it.
        if let Ok(line) = serde_json::to_string(&record) {
            let _ = writeln!(self.sink, "{line}");
        }
    }
}

impl<T: Transport, W: Write> Read for TeeTransport<T, W> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.record(Direction::Rx, &buf[..n]);
        Ok(n)
    }
}

impl<T: Transport, W: Write> Write for TeeTransport<T, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.record(Direction::Tx, &buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl<T: Transport, W: Write> Transport for TeeTransport<T, W> {
    fn describe(&self) -> String {
        format!("{} (recording)", self.inner.describe())
    }
}
