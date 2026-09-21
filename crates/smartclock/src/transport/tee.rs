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

/// How many bytes a coalesced run may hold before it is written out.
///
/// Comfortably larger than a status screen, so ordinary traffic still
/// records as one line per direction.
const MAX_RUN: usize = 8192;

/// Wraps a transport and writes every read and write to a JSONL sink.
///
/// Consecutive bytes travelling the same way are coalesced into one
/// record.  The receiver echoes a character at a time, so recording
/// each read separately turned a single probe run into 17,500 records
/// and 700 KB; coalescing brings that to a few hundred lines that can
/// actually be read.
#[derive(Debug)]
pub struct TeeTransport<T: Transport, W: Write> {
    inner: T,
    sink: BufWriter<W>,
    started: Instant,
    /// Bytes accumulated since the direction last changed, with the
    /// time the first of them moved.
    run: Option<(Direction, f64, Vec<u8>)>,
}

impl<T: Transport, W: Write> TeeTransport<T, W> {
    /// Begin recording `inner` into `sink`.
    pub fn new(inner: T, sink: W) -> Self {
        Self {
            inner,
            sink: BufWriter::new(sink),
            started: Instant::now(),
            run: None,
        }
    }

    /// Write out any pending run and flush.  Dropping the tee does this
    /// too; call it explicitly when a write failure should be noticed.
    pub fn finish(&mut self) -> Result<()> {
        self.emit();
        self.sink.flush()?;
        Ok(())
    }

    fn record(&mut self, dir: Direction, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        // A run is flushed when it reaches this size as well as when
        // the direction changes, so a long one-directional stream --
        // the status screen, a full log dump -- cannot sit unwritten
        // and growing.
        if self
            .run
            .as_ref()
            .is_some_and(|(_, _, buf)| buf.len() >= MAX_RUN)
        {
            self.emit();
        }
        match &mut self.run {
            Some((running, _, buf)) if *running == dir => buf.extend_from_slice(bytes),
            _ => {
                self.emit();
                let at = self.started.elapsed().as_secs_f64();
                self.run = Some((dir, at, bytes.to_vec()));
            }
        }
    }

    /// Write the pending run, if any, as one record.
    fn emit(&mut self) {
        let Some((dir, at, bytes)) = self.run.take() else {
            return;
        };
        let record = Record::new(at, dir, &bytes);
        // A transcript is diagnostic, so failing to write one must not
        // take down the session producing it.
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

impl<T: Transport, W: Write> Drop for TeeTransport<T, W> {
    fn drop(&mut self) {
        // finish() consumes self, so a caller that just drops the tee
        // would otherwise lose the last run.
        self.emit();
        let _ = self.sink.flush();
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
