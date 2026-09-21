//! Copying the receiver's own record-keeping into the log.
//!
//! Two things the receiver writes down for itself: a queue of errors it
//! has raised, and a diagnostic log of events it thought worth keeping.
//! Both were previously left where they were, which for the error queue
//! meant they were never read at all and for the diagnostic log meant
//! the daemon recorded how many entries there were and not what any of
//! them said.
//!
//! Neither survives being left alone.  The error queue is finite and
//! discards what it cannot hold; the diagnostic log holds 222 entries
//! and then stops recording.  Copying them out is the only way the
//! receiver's own account of its faults outlives the receiver.
//!
//! This runs on the thread that owns the database rather than in the
//! poll schedule, because it is the only arrangement with no window in
//! which an entry has been taken from the receiver and not yet written
//! down.  Reading an error queue entry removes it; if the read and the
//! write are on different threads with a channel between them, a
//! shutdown in that gap loses the entry for good.

use anyhow::Result;
use smartclock::command::CommandId;
use smartclock::command::Dialect;
use smartclock::parse;
use smartclock::task::Handle;
use std::time::Duration;

use crate::db::Log;

/// How long a journal query waits before being given up on.
///
/// Short next to the client timeout: nothing is waiting on these, and a
/// journal pass that blocks holds up the writing of snapshots behind
/// it.
const TIMEOUT: Duration = Duration::from_secs(5);

/// How many error queue entries one pass will take.
///
/// The receiver's queue is bounded, so a healthy one empties long
/// before this. The cap is for the unhealthy case: a receiver refusing
/// everything refills its queue as fast as it is drained, and a loop
/// with no bound would never return to writing snapshots.
const ERRORS_PER_PASS: usize = 64;

/// How many diagnostic log entries one pass will read.
///
/// Each is a separate query, and a full log is 222 of them -- about
/// half a minute of wire time at 19200, which is not something to do
/// between two snapshots. Spread over passes the backfill finishes in
/// a few minutes and nothing else notices.
const ENTRIES_PER_PASS: usize = 16;

/// Copies the error queue and the diagnostic log into the database.
#[derive(Debug, Default)]
pub(crate) struct Journal {
    /// The inclusive span of diagnostic log entry numbers already
    /// copied, or `None` before the first pass.
    ///
    /// A span rather than a high-water mark because the copy runs in
    /// both directions: new entries first, since they are why anyone is
    /// watching, and then backwards through the history that was
    /// already there when the daemon started.
    copied: Option<(i64, i64)>,
}

impl Journal {
    /// Take whatever the receiver has been keeping to itself.
    ///
    /// Errors are reported rather than propagated: the journal is a
    /// side errand of the thread that writes snapshots, and a receiver
    /// that will not answer these must not stop that thread.
    pub(crate) fn pass(&mut self, handle: &Handle, dialect: Dialect, log: &mut Log) {
        if let Err(e) = self.drain_errors(handle, dialect, log) {
            eprintln!("smartclockd: could not read the error queue: {e:#}");
        }
        if let Err(e) = self.copy_log(handle, dialect, log) {
            eprintln!("smartclockd: could not read the diagnostic log: {e:#}");
        }
    }

    /// Read the error queue empty, recording what it held.
    ///
    /// Destructive, and deliberately so: reading is the only way to
    /// take an entry, and an entry left in the queue is lost when the
    /// queue fills. Nothing else in this daemon reads it.
    fn drain_errors(&mut self, handle: &Handle, dialect: Dialect, log: &mut Log) -> Result<()> {
        for _ in 0..ERRORS_PER_PASS {
            let line = ask(handle, dialect, CommandId::Error, None)?;
            let entry = parse::error_entry(&line)?;
            if entry.is_empty() {
                return Ok(());
            }
            log.record_error(entry.code, &entry.message)?;
            eprintln!(
                "smartclockd: the receiver reported {} {}",
                entry.code, entry.message
            );
        }
        eprintln!(
            "smartclockd: the error queue still had entries after {ERRORS_PER_PASS}; \
             leaving the rest for the next pass"
        );
        Ok(())
    }

    /// Copy any diagnostic log entries not already held.
    fn copy_log(&mut self, handle: &Handle, dialect: Dialect, log: &mut Log) -> Result<()> {
        let count = parse::int(&ask(handle, dialect, CommandId::LogCount, None)?)?;
        if count < 1 {
            return Ok(());
        }
        // An entry count that has gone backwards means the log was
        // cleared and is being numbered again from one, so what we hold
        // says nothing about what is there now.
        let (mut low, mut high) = match self.copied {
            Some((low, high)) if high <= count => (low, high),
            // An empty span at the top, so the backfill below starts at
            // the newest entry and works down.
            _ => (count + 1, count),
        };

        let mut budget = ENTRIES_PER_PASS;
        // Upwards first: a new entry is why anyone is reading this.
        while budget > 0 && high < count {
            self.copy_entry(handle, dialect, log, high + 1)?;
            high += 1;
            budget -= 1;
        }
        // Then backwards through whatever was already in the receiver
        // when the daemon started.
        while budget > 0 && low > 1 {
            self.copy_entry(handle, dialect, log, low - 1)?;
            low -= 1;
            budget -= 1;
        }
        self.copied = Some((low, high));
        Ok(())
    }

    /// Read one diagnostic log entry and record it.
    fn copy_entry(
        &self,
        handle: &Handle,
        dialect: Dialect,
        log: &mut Log,
        entry: i64,
    ) -> Result<()> {
        let line = ask(handle, dialect, CommandId::LogRead, Some(entry))?;
        let text = parse::string(&line).unwrap_or(line.as_str());
        let (stamp, message) = split_entry(text);
        log.record_log_entry(entry, stamp, message)?;
        Ok(())
    }
}

/// Send one logical query and return its single reply line.
///
/// Through the dialect table rather than with the SCPI written out
/// here, so these queries are spelled the way every other one is and a
/// receiver whose tree differs is handled in the one place that knows
/// about it.
fn ask(handle: &Handle, dialect: Dialect, id: CommandId, argument: Option<i64>) -> Result<String> {
    let spec = dialect
        .spec(id)
        .ok_or_else(|| anyhow::anyhow!("{} does not have {id:?}", dialect.name()))?;
    let scpi = match argument {
        Some(value) => format!("{} {value}", spec.scpi),
        None => spec.scpi.to_owned(),
    };
    let reply = handle.request_within(scpi, TIMEOUT)?;
    Ok(reply.one_line("a single line")?.to_owned())
}

/// Split `Log NNN:YYYYMMDD.HH:MM:SS: message` into its timestamp and
/// its message.
///
/// The entry number is dropped: it was in the request. The timestamp is
/// kept as written rather than parsed, because it comes from the
/// receiver's own calendar and carries whatever GPS week rollover that
/// calendar has -- a 58503A on firmware 3704-C stamps its log 1024
/// weeks early, and parsing that into a date would bury the evidence.
///
/// Anything that does not fit the documented shape is kept whole as the
/// message. An entry whose text we do not recognise is still the
/// receiver telling us something.
fn split_entry(text: &str) -> (Option<&str>, &str) {
    let whole = (None, text);
    let Some(rest) = text.strip_prefix("Log ") else {
        return whole;
    };
    let Some((_number, rest)) = rest.split_once(':') else {
        return whole;
    };
    let rest = rest.trim_start();
    // The timestamp has two colons of its own, so the one that ends it
    // is the third.
    let mut colons = rest.match_indices(':').map(|(at, _)| at);
    let (Some(_), Some(_), Some(end)) = (colons.next(), colons.next(), colons.next()) else {
        return whole;
    };
    (Some(&rest[..end]), rest[end + 1..].trim())
}

#[cfg(test)]
mod tests {
    use super::split_entry;

    #[test]
    fn an_entry_splits_into_its_timestamp_and_its_message() {
        let (stamp, message) =
            split_entry("Log 222:20050727.06:17:34: Holdover started, not tracking GPS");
        assert_eq!(stamp, Some("20050727.06:17:34"));
        assert_eq!(message, "Holdover started, not tracking GPS");
    }

    #[test]
    fn a_message_containing_a_colon_keeps_it() {
        let (stamp, message) = split_entry("Log 7:20050727.06:17:34: Power on: cold start");
        assert_eq!(stamp, Some("20050727.06:17:34"));
        assert_eq!(message, "Power on: cold start");
    }

    /// The two ends of a real 58503A's log, read from the instrument.
    ///
    /// Note the padding: the receiver writes `Log 001` and `Log 222`,
    /// so a reader keying on the number's width would be wrong at one
    /// end or the other.  The number is dropped here, which is why it
    /// does not matter.
    #[test]
    fn the_entries_a_58503a_actually_returns_split() {
        let (stamp, message) =
            split_entry("Log 222:20050727.06:17:34: Holdover started, not tracking GPS");
        assert_eq!(stamp, Some("20050727.06:17:34"));
        assert_eq!(message, "Holdover started, not tracking GPS");

        let (stamp, message) = split_entry("Log 001:20050528.00:00:00: Log cleared");
        assert_eq!(stamp, Some("20050528.00:00:00"));
        assert_eq!(message, "Log cleared");
    }

    #[test]
    fn an_entry_in_an_unexpected_shape_is_kept_whole() {
        // Better a row saying something we cannot parse than no row:
        // the receiver's firmware is the authority on this format and
        // the manual is not exhaustive about it.
        for text in ["something else entirely", "Log 4", "Log 4:no timestamp"] {
            let (stamp, message) = split_entry(text);
            assert_eq!(stamp, None);
            assert_eq!(message, text);
        }
    }
}
