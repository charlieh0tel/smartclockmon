//! Copying the receiver's own record-keeping into the log.
//!
//! Two things the receiver writes down for itself: a queue of errors it
//! has raised, and a diagnostic log of events it thought worth keeping.
//! Both were previously left where they were, which for the error queue
//! meant nothing ever read an error the receiver raised unprompted, and
//! for the diagnostic log meant the daemon recorded how many entries
//! there were and not what any of them said.
//!
//! Neither survives being left alone.  The error queue is finite and
//! discards what it cannot hold; the diagnostic log holds 222 entries
//! and then stops recording -- the development unit's has been full,
//! and silently recording nothing, since March 2025.  Copying them out
//! is the only way the receiver's own account of its faults outlives
//! the receiver.
//!
//! This runs on the thread that owns the database rather than in the
//! poll schedule, so the write follows the read with nothing between
//! them: reading an error queue entry is what removes it, and a channel
//! between the read and the write is a gap a shutdown can lose it in.
//! That is not the same as a guarantee.  A reply that times out after
//! the command reached the wire takes an entry nobody receives, and
//! `Session::query` reads one entry of its own after any error prompt
//! to explain it -- including prompts caused by the queries here.  What
//! can be promised is narrower: an entry this module reads is written
//! down before it asks for another, and one it cannot parse is written
//! down raw rather than dropped.

use anyhow::Result;
use smartclock::command::CommandId;
use smartclock::command::Dialect;
use smartclock::parse;
use smartclock::task::Handle;
use std::time::Duration;
use std::time::Instant;

use crate::db::Log;

/// How long a journal query waits before being given up on.
///
/// Short next to the client timeout: nothing is waiting on these, and a
/// journal pass that blocks holds up the writing of snapshots behind
/// it.
const TIMEOUT: Duration = Duration::from_secs(5);

/// How long a whole pass may take.
///
/// The per-query timeout is not enough on its own.  A receiver refusing
/// everything answers slowly and refills its error queue as fast as it
/// is drained, so the caps below could between them spend eighty-one
/// timeouts -- nearly seven minutes -- in one pass, during which this
/// thread writes no snapshots and no audit rows, and every row behind
/// it is stamped with the time it was finally written rather than the
/// time it happened.  Nothing here is worth that.
const PASS_BUDGET: Duration = Duration::from_secs(20);

/// How many error queue entries one pass will take.
const ERRORS_PER_PASS: usize = 64;

/// How many diagnostic log entries one pass will read.
///
/// Each is a separate query, and a full log is 222 of them -- about
/// half a minute of wire time at 19200, which is not something to do
/// between two snapshots.  Spread over passes the backfill finishes in
/// a few minutes and nothing else notices.
const ENTRIES_PER_PASS: usize = 16;

/// How many times one entry may fail before it is stepped over.
///
/// Abandoning the pass on any failure meant a single entry the receiver
/// would not give up stopped the backfill for good: every pass retried
/// it, failed, and never reached the entries below.  Retrying a few
/// times covers a link that blipped; past that the entry is the
/// problem, and the rest of the log is worth more than it is.
const ATTEMPTS_PER_ENTRY: u32 = 3;

/// The code recorded for an error queue entry that would not parse.
///
/// Zero means "no error" and every real code fits a documented range,
/// so this cannot be mistaken for one the receiver gave.
const UNPARSED: i32 = i32::MIN;

/// Copies the error queue and the diagnostic log into the database.
#[derive(Debug, Default)]
pub(crate) struct Journal {
    /// Which receiver [`Journal::copied`] describes.
    ///
    /// The span means nothing without it.  Kept across passes but not
    /// across receivers: a unit swapped for one with a longer log would
    /// otherwise have its first two hundred entries judged copied on
    /// the strength of the previous unit's progress, and never read.
    receiver: Option<i64>,
    /// The inclusive span of diagnostic log entry numbers already
    /// copied, or `None` before the first pass for this receiver.
    ///
    /// A span rather than a high-water mark because the copy runs in
    /// both directions: new entries first, since they are why anyone is
    /// watching, and then backwards through the history that was
    /// already there.
    copied: Option<(i64, i64)>,
    /// An entry that will not come, and how many times it has not.
    stuck: Option<(i64, u32)>,
}

impl Journal {
    /// Take whatever the receiver has been keeping to itself.
    ///
    /// Errors are reported rather than propagated: the journal is a
    /// side errand of the thread that writes snapshots, and a receiver
    /// that will not answer these must not stop that thread.
    pub(crate) fn pass(&mut self, handle: &Handle, dialect: Dialect, receiver: i64, log: &mut Log) {
        if self.receiver != Some(receiver) {
            // Resume where the database says this receiver got to,
            // rather than from nothing: at sixteen entries a minute a
            // full log takes a quarter of an hour, so a daemon
            // restarted more often than that re-read the newest
            // entries for ever and never reached the oldest.
            self.copied = match log.log_span(receiver) {
                Ok(span) => span,
                Err(e) => {
                    eprintln!("smartclockd: could not read the copied log span: {e:#}");
                    None
                }
            };
            self.receiver = Some(receiver);
            self.stuck = None;
        }
        let deadline = Instant::now() + PASS_BUDGET;
        if let Err(e) = self.drain_errors(handle, dialect, log, deadline) {
            eprintln!("smartclockd: could not read the error queue: {e:#}");
        }
        if let Err(e) = self.copy_log(handle, dialect, log, deadline) {
            eprintln!("smartclockd: could not read the diagnostic log: {e:#}");
        }
    }

    /// Read the error queue empty, recording what it held.
    ///
    /// Destructive, and deliberately so: reading is the only way to
    /// take an entry, and an entry left in the queue is lost when the
    /// queue fills.
    fn drain_errors(
        &mut self,
        handle: &Handle,
        dialect: Dialect,
        log: &mut Log,
        deadline: Instant,
    ) -> Result<()> {
        for _ in 0..ERRORS_PER_PASS {
            if Instant::now() >= deadline {
                return Ok(());
            }
            let line = ask(handle, dialect, CommandId::Error, None)?;
            // The read has already taken the entry, so a reply that
            // will not parse is recorded as it arrived rather than
            // dropped.  It is gone from the receiver either way, and a
            // row saying something we cannot read beats no row.
            let Ok(entry) = parse::error_entry(&line) else {
                eprintln!("smartclockd: unreadable error queue entry {line:?}");
                log.record_error(UNPARSED, line.trim())?;
                continue;
            };
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
    fn copy_log(
        &mut self,
        handle: &Handle,
        dialect: Dialect,
        log: &mut Log,
        deadline: Instant,
    ) -> Result<()> {
        let count = parse::int(&ask(handle, dialect, CommandId::LogCount, None)?)?;
        if count < 1 {
            return Ok(());
        }
        let (mut low, mut high) = seed(self.copied, count);
        let mut fresh = 0usize;
        for entry in wanted(low, high, count, ENTRIES_PER_PASS) {
            if Instant::now() >= deadline {
                break;
            }
            match self.copy_entry(handle, dialect, log, entry) {
                Ok(new) => {
                    self.stuck = None;
                    fresh += usize::from(new);
                }
                Err(e) => {
                    let attempts = match self.stuck {
                        Some((at, attempts)) if at == entry => attempts + 1,
                        _ => 1,
                    };
                    eprintln!(
                        "smartclockd: diagnostic log entry {entry} would not read \
                         (attempt {attempts} of {ATTEMPTS_PER_ENTRY}): {e:#}"
                    );
                    if attempts < ATTEMPTS_PER_ENTRY {
                        // Leave the span short of it, so the next pass
                        // asks for the same entry again.
                        self.stuck = Some((entry, attempts));
                        break;
                    }
                    eprintln!("smartclockd: giving up on diagnostic log entry {entry}");
                    self.stuck = None;
                }
            }
            // Advanced whether or not the entry arrived, so an entry
            // given up on cannot stop the walk reaching the ones past
            // it.  The gap is in the database, and a walk of the whole
            // log fills it.
            if entry > high {
                high = entry;
            } else {
                low = entry;
            }
        }
        // Committed even when the walk stopped early, so that the work
        // a cut-short pass did is not done again by the next one.
        self.copied = Some((low, high));
        if fresh > 0 {
            eprintln!(
                "smartclockd: copied {fresh} diagnostic log entries; {} of {count} now held",
                high - low + 1
            );
        }
        Ok(())
    }

    /// Read one diagnostic log entry and record it, reporting whether
    /// the database did not already hold it.
    fn copy_entry(
        &self,
        handle: &Handle,
        dialect: Dialect,
        log: &mut Log,
        entry: i64,
    ) -> Result<bool> {
        let line = ask(handle, dialect, CommandId::LogRead, Some(entry))?;
        let text = parse::string(&line).unwrap_or(line.as_str());
        let (stamp, message) = split_entry(text);
        log.record_log_entry(entry, stamp, message)
    }
}

/// The span to start this pass from.
///
/// An entry count below the highest entry already held means the log
/// was cleared and is being numbered again from one, so what is held
/// describes entries that no longer exist.  An empty span at the top
/// makes the walk begin at the newest entry and work down.
fn seed(copied: Option<(i64, i64)>, count: i64) -> (i64, i64) {
    match copied {
        Some((low, high)) if high <= count => (low, high),
        _ => (count + 1, count),
    }
}

/// Which entries to read, in order, and no more than `budget` of them.
///
/// Upwards from the span first, because a new entry is why anyone is
/// reading this, and then downwards through whatever was already in the
/// receiver when the daemon started.  In that order so that taking any
/// prefix leaves the span contiguous.
fn wanted(low: i64, high: i64, count: i64, budget: usize) -> Vec<i64> {
    (high + 1..=count)
        .chain((1..low).rev())
        .take(budget)
        .collect()
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
/// The entry number is dropped: it was in the request, and the receiver
/// pads it at one end of its log and not the other.  The timestamp is
/// kept as written rather than parsed, because it comes from the
/// receiver's own calendar and carries whatever GPS week rollover that
/// calendar has -- a 58503A on firmware 3704-C stamps its log 1024
/// weeks early, and parsing that into a date would bury the evidence.
///
/// Anything that does not fit the documented shape is kept whole as the
/// message.  An entry whose text we do not recognise is still the
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
    use super::seed;
    use super::split_entry;
    use super::wanted;

    /// One pass as `copy_log` runs it: seed the span, take a pass
    /// worth of entries, advance the span the way the loop does.
    ///
    /// The arithmetic is where every failure mode of this module lives
    /// and all of them are silent, so it is worth exercising apart from
    /// the receiver it would otherwise need.
    fn pass(copied: Option<(i64, i64)>, count: i64, budget: usize) -> (Vec<i64>, (i64, i64)) {
        let (mut low, mut high) = seed(copied, count);
        let entries = wanted(low, high, count, budget);
        for &entry in &entries {
            if entry > high {
                high = entry;
            } else {
                low = entry;
            }
        }
        (entries, (low, high))
    }

    #[test]
    fn a_first_pass_starts_at_the_newest_entry_and_works_down() {
        let (entries, span) = pass(None, 222, 4);
        assert_eq!(entries, vec![222, 221, 220, 219]);
        assert_eq!(span, (219, 222));
    }

    #[test]
    fn the_backfill_resumes_where_it_stopped() {
        let (entries, span) = pass(Some((219, 222)), 222, 4);
        assert_eq!(entries, vec![218, 217, 216, 215]);
        assert_eq!(span, (215, 222));
    }

    #[test]
    fn new_entries_are_read_before_the_backfill_continues() {
        // Two arrived while the backfill was still working down.  The
        // new ones come first: they are why anyone is watching, and the
        // old ones are not going anywhere.
        let (entries, span) = pass(Some((100, 222)), 224, 4);
        assert_eq!(entries, vec![223, 224, 99, 98]);
        assert_eq!(span, (98, 224));
    }

    #[test]
    fn a_finished_copy_asks_for_nothing() {
        let (entries, span) = pass(Some((1, 222)), 222, 16);
        assert!(entries.is_empty());
        assert_eq!(span, (1, 222));
    }

    #[test]
    fn a_cleared_log_is_copied_again_from_the_top() {
        // The count going backwards is the only sign that the log was
        // cleared: the entry numbers restart at one and would otherwise
        // read as entries already held.
        let (entries, span) = pass(Some((1, 222)), 3, 16);
        assert_eq!(entries, vec![3, 2, 1]);
        assert_eq!(span, (1, 3));
    }

    #[test]
    fn a_longer_log_on_another_unit_is_not_taken_for_copied() {
        // The span belongs to a receiver, and `Journal::pass` clears it
        // when the receiver changes.  Kept -- which it was, until a
        // review caught it -- a unit swapped for one with a longer log
        // had entries 1..222 judged copied on the strength of the
        // previous unit's progress and never read at all.
        let (entries, _) = pass(Some((1, 222)), 300, 4);
        assert_eq!(
            entries,
            vec![223, 224, 225, 226],
            "carrying a span across receivers skips the new unit's history"
        );
        let (entries, span) = pass(None, 300, 4);
        assert_eq!(entries, vec![300, 299, 298, 297]);
        assert_eq!(span, (297, 300));
    }

    #[test]
    fn a_log_of_one_entry_is_copied_once_and_not_again() {
        let (entries, span) = pass(None, 1, 16);
        assert_eq!(entries, vec![1]);
        assert_eq!(span, (1, 1));
        assert!(pass(Some(span), 1, 16).0.is_empty());
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
    fn a_message_containing_a_colon_keeps_it() {
        let (stamp, message) = split_entry("Log 7:20050727.06:17:34: Power on: cold start");
        assert_eq!(stamp, Some("20050727.06:17:34"));
        assert_eq!(message, "Power on: cold start");
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
