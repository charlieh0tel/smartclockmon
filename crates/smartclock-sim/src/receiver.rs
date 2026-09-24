//! The simulated receiver's state and its answers.

use std::collections::VecDeque;

use smartclock::command::CommandId;
use smartclock::command::Dialect;
use smartclock::types::EfcPercent;

/// What the receiver sends back, before framing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// Response lines, if any.  A setter returns none.
    pub lines: Vec<String>,
    /// Whether the command was accepted.
    pub accepted: bool,
}

impl Answer {
    /// A command that produced no output.
    fn silent() -> Self {
        Self {
            lines: Vec::new(),
            accepted: true,
        }
    }

    /// A single line of output.
    fn line(text: impl Into<String>) -> Self {
        Self {
            lines: vec![text.into()],
            accepted: true,
        }
    }
}

/// How many errors the queue holds: thirty, 097-59551-02 5-31.  Public
/// so a test can state the bound it expects rather than repeating the
/// number.
pub const MAX_ERRORS: usize = 30;

/// SCPI's "Queue overflow", which a full queue reports in place of its
/// last entry.
const QUEUE_OVERFLOW_CODE: i32 = -350;
const QUEUE_OVERFLOW_MESSAGE: &str = "Queue overflow";

/// Today, less one GPS epoch, as firmware predating the 2019 wrap
/// reports it.
///
/// Derived rather than hardcoded.  A fixed date is only exactly one
/// epoch behind on one day of the year, and the rollover check accepts
/// only whole multiples -- so a constant here quietly stopped
/// exercising the rollover path the day after it was written, which is
/// precisely the path the date display exists for.
fn rolled_back_date() -> jiff::civil::Date {
    const EPOCH_DAYS: i32 = 1024 * 7;
    // UTC, as the receiver reports and as the daemon's rollover check
    // compares against.  The local date differs from it for part of
    // every day, and on those hours the offset is not a whole epoch, so
    // the check correctly declines to call it a rollover.
    jiff::Timestamp::now()
        .to_zoned(jiff::tz::TimeZone::UTC)
        .date()
        .saturating_sub(jiff::Span::new().days(EPOCH_DAYS))
}

/// How many diagnostic log entries the simulated receiver holds.  The
/// real one tops out at 222.
const LOG_ENTRIES: i64 = 222;

/// The alarm enable register as a 58503A reports it: questionable
/// summary and operation summary, which is the documented preset.
const ALARM_ENABLE: u16 = (1 << 3) | (1 << 7);

/// Where the instrument starts saying its log is nearly full.
const LOG_ALMOST_FULL_AT: i64 = 200;

/// What the instrument writes as entry one after a clear, in its own
/// rolled-back calendar.
const CLEARED_ENTRY: &str = "\"Log 001:20050728.00:00:00: Log cleared\"";

/// One diagnostic log entry in the receiver's own format:
/// `"Log NNN: YYYYMMDD.HH:MM:SS: <message>"`.  097-59551-02 5-34.
///
/// The messages repeat in a short cycle rather than being all the same,
/// so a reader that keys entries by their text rather than by their
/// number is caught here instead of in the field.
///
/// `era` counts refills since the log was built, and moves the date,
/// so a log cleared and refilled to the same count reads differently.
fn log_entry(entry: i64, era: i64) -> String {
    const MESSAGES: [&str; 4] = [
        "Holdover started, not tracking GPS",
        "Holdover ended",
        "Position survey complete",
        "Power on",
    ];
    let minute = entry % 60;
    let day = 27 + era;
    // Zero padded to three, as the instrument writes it.
    format!(
        "\"Log {entry:03}:200507{day:02}.06:{minute:02}:34: {}\"",
        MESSAGES[(entry as usize) % MESSAGES.len()]
    )
}

/// A status screen with plausible contents, in the layout firmware
/// 3704-C uses: an `SS` column and underscore padding.
const SCREEN: &str = include_str!("screen.txt");

/// A simulated 58503A.
///
/// Values drift a little with each poll, because a simulator that
/// returns a constant hides exactly the bugs worth finding: a trend
/// that never moves, a chart that cannot scale itself, an average that
/// is indistinguishable from a single sample.
#[derive(Debug)]
pub struct Receiver {
    /// What `*IDN?` returns.
    pub identity: String,
    /// Raw EFC, the 20-bit value.
    pub efc_raw: u32,
    /// Internal temperature, degrees Celsius.
    pub temperature: f64,
    /// 1 PPS against GPS, in seconds.
    pub time_interval: f64,
    /// Hardware condition register.
    pub hardware: u16,
    /// Whether the receiver is in holdover.
    pub holdover: bool,
    /// Which command tree to answer on.
    pub dialect: Dialect,
    /// How many commands have been answered.
    ticks: u64,
    /// What the drifting values move around.  Captured when the
    /// receiver is built, so a receiver configured with a railed EFC
    /// stays railed instead of being reset on the first command.
    base: Base,
    /// Errors waiting to be read, oldest first.
    errors: VecDeque<(i32, String)>,
    /// How many diagnostic log entries are held.  Falls to one when
    /// the log is cleared, as the instrument's does.
    log_entries: i64,
    /// Whether the log has been cleared, so entry one reads as the
    /// receiver's own "Log cleared" marker rather than as history.
    log_cleared: bool,
    /// How many times the log has been refilled; see [`log_entry`].
    log_era: i64,
    /// An entry that reads as it did before the refill.
    log_unchanged: Option<i64>,
    /// Event registers, latched until read.
    ///
    /// Modelled rather than answered from the conditions, because the
    /// difference is the whole character of an event register: it holds
    /// a transition until somebody takes it, and taking it is what
    /// clears it.  A simulator that answered these from the current
    /// condition would return the same value for ever and hide every
    /// bug in code that assumes reading empties them.
    events: Events,
    /// Refuse every command the receiver would otherwise answer.
    ///
    /// A receiver that says no to everything is not a broken link, and
    /// the daemon has to tell the two apart: see
    /// [`Receiver::refusing`].
    refuse_everything: bool,
    /// Commands answered with a line that parses as nothing.
    ///
    /// For making a poll fail partway, after the fields read before
    /// the garbled one: the case where half-read values could leak out
    /// as though the whole poll had succeeded.  Matched ignoring case.
    pub garbled: Vec<String>,
    /// Whether the receiver has ever had a fix.
    ///
    /// Until it has, everything derived from GPS -- the date, the time,
    /// the position, the time interval -- is refused with -230, and
    /// there is no way to hurry it.  A cold unit is not a broken one,
    /// and it is the state a receiver spends its first minutes in, so
    /// it is the state most likely to be met and least likely to be
    /// tested against.
    no_fix: bool,
}

/// The latched event registers, by the same short names the daemon
/// records them under.
#[derive(Debug, Clone, Copy, Default)]
struct Events {
    operation: u16,
    questionable: u16,
    hardware: u16,
    holdover: u16,
    powerup: u16,
}

/// The values drift moves around.
#[derive(Debug, Clone, Copy)]
struct Base {
    efc_raw: u32,
    temperature: f64,
    time_interval: f64,
}

impl Receiver {
    /// Capture the current values as what drift should move around.
    ///
    /// Called after construction, so a caller can set any field and
    /// have the drift respect it.
    fn settle(mut self) -> Self {
        self.base = Base {
            efc_raw: self.efc_raw,
            temperature: self.temperature,
            time_interval: self.time_interval,
        };
        self
    }
}

impl Default for Receiver {
    fn default() -> Self {
        Self {
            identity: "HEWLETT-PACKARD,58503A,0000A00000,3704-C".to_owned(),
            efc_raw: 713_587,
            temperature: 37.40,
            time_interval: -4.8e-9,
            hardware: 0,
            holdover: false,
            dialect: Dialect::Hp58503,
            ticks: 0,
            refuse_everything: false,
            garbled: Vec::new(),
            no_fix: false,
            base: Base {
                efc_raw: 713_587,
                temperature: 37.40,
                time_interval: -4.8e-9,
            },
            errors: VecDeque::new(),
            events: Events::default(),
            log_entries: LOG_ENTRIES,
            log_cleared: false,
            log_era: 0,
            log_unchanged: None,
        }
    }
}

impl Receiver {
    /// A receiver reporting a fault, for testing the paths that only
    /// run when something is wrong.
    pub fn faulty() -> Self {
        Self {
            // Bit 6 is EFC near full scale, bit 7 is at full scale.
            hardware: 1 << 6,
            efc_raw: 1_040_000,
            holdover: true,
            ..Self::default()
        }
        .settle()
    }

    /// A receiver that has never had a fix, as one is for its first
    /// minutes from cold and for as long as it cannot see the sky.
    ///
    /// Everything derived from GPS is refused with -230.  A poll must
    /// come back with the rest of the tier regardless: on a real
    /// Z3805A a bare `?` on the date meant the slow tier never
    /// completed once, so the log count, the oscillator tempco and the
    /// powerup register -- which sit after it -- reported themselves
    /// missing when they had never been asked.
    pub fn cold() -> Self {
        Self {
            no_fix: true,
            ..Self::default()
        }
        .settle()
    }

    /// A receiver that refuses every command it would otherwise
    /// answer, with -113.
    ///
    /// For the difference between a receiver saying no and a link that
    /// has gone: only the second is worth reopening the port over, and
    /// treating the first as the second put the daemon in a five-second
    /// reconnect loop against healthy hardware.  Reading the error
    /// queue still works, or nothing could learn why.
    pub fn refusing() -> Self {
        Self {
            refuse_everything: true,
            ..Self::default()
        }
        .settle()
    }

    /// Answer one command.
    pub fn respond(&mut self, command: &str) -> Answer {
        self.ticks += 1;
        self.drift();

        let command = command.trim();
        if command.is_empty() {
            return Answer::silent();
        }
        if self.garbled.iter().any(|g| g.eq_ignore_ascii_case(command)) {
            return Answer::line("#not a reading#");
        }
        if let Some(answer) = self.common(command) {
            return answer;
        }
        // The error queue is answered before the table, since reading it
        // is how a caller clears it.
        if eq(command, ":SYSTem:ERRor?") {
            let (code, message) = self
                .errors
                .pop_front()
                .unwrap_or_else(|| (0, "No error".to_owned()));
            return Answer::line(format!("{code:+},\"{message}\""));
        }

        let (header, argument) = split(command);
        let found = self
            .dialect
            .specs()
            .iter()
            .find(|s| eq(s.scpi, command) || eq(s.scpi, header));
        match found {
            Some(_) if self.refuse_everything => self.reject(-113, "Undefined header"),
            Some(spec) => self.answer(spec.id, argument),
            None => self.reject(-113, "Undefined header"),
        }
    }

    /// The alarm condition register: which groups have something
    /// latched.
    ///
    /// Summarises the event registers, so it goes out by itself when
    /// they are read -- which is the property that makes reading them
    /// take the operator's lamp away, and the reason the daemon polls
    /// this instead.
    fn alarm_bits(&self) -> u16 {
        const QUESTIONABLE: u16 = 1 << 3;
        const MASTER: u16 = 1 << 6;
        const OPERATION: u16 = 1 << 7;
        let mut bits = 0;
        if self.events.questionable != 0 {
            bits |= QUESTIONABLE;
        }
        if self.events.operation != 0
            || self.events.hardware != 0
            || self.events.holdover != 0
            || self.events.powerup != 0
        {
            bits |= OPERATION;
        }
        if bits & ALARM_ENABLE != 0 {
            bits |= MASTER;
        }
        bits
    }

    /// Read one event register and clear it, as the instrument does.
    fn take_event(&mut self, which: fn(&mut Events) -> &mut u16) -> u16 {
        std::mem::take(which(&mut self.events))
    }

    /// Latch an event, as a transition would.
    ///
    /// Public so a test can make something happen and then check that
    /// reading it once returns it and reading it twice does not.
    pub fn raise_event(&mut self, register: &str, bits: u16) {
        let field = match register {
            "operation" => &mut self.events.operation,
            "questionable" => &mut self.events.questionable,
            "hardware" => &mut self.events.hardware,
            "holdover" => &mut self.events.holdover,
            "powerup" => &mut self.events.powerup,
            _ => return,
        };
        *field |= bits;
    }

    /// Clear the log and fill it again to the same count with different
    /// entries, as happens while nothing is watching.
    ///
    /// The count alone cannot show that this happened, which is the
    /// case a reader has to catch before erasing what it thinks it
    /// holds.
    pub fn refill_log(&mut self) {
        self.log_era += 1;
        self.log_cleared = false;
        self.log_entries = LOG_ENTRIES;
    }

    /// As [`Receiver::refill_log`], with entry `same` reading exactly as
    /// it did before: the receiver repeats a power-on stamp and message,
    /// so one matching entry is not proof of the same log.
    pub fn refill_log_matching(&mut self, same: i64) {
        self.refill_log();
        self.log_unchanged = Some(same);
    }

    /// How many diagnostic log entries are held.
    pub fn log_entries(&self) -> i64 {
        self.log_entries
    }

    /// Put an error in the queue that no command of ours caused.
    ///
    /// The receiver raises errors on its own -- a failed self test, a
    /// reference lost -- and those are the ones worth having, because
    /// an error raised by a command we sent is read back by the session
    /// to explain that command's failure and never reaches the queue
    /// again.  Seeding one is the only way to exercise the code that
    /// collects the rest.
    pub fn queue_error(&mut self, code: i32, message: &str) {
        self.errors.push_back((code, message.to_owned()));
    }

    /// The operation condition register this receiver would report.
    ///
    /// Assembled from the state the simulator already keeps rather than
    /// stored separately, so it cannot disagree with the mode and the
    /// holdover flag that the other commands answer from.
    fn operation_bits(&self) -> u16 {
        const POWERUP_SUMMARY: u16 = 1;
        const LOCKED: u16 = 1 << 1;
        const POSITION_HOLD: u16 = 1 << 3;
        const REFERENCE_VALID: u16 = 1 << 4;
        const LOG_ALMOST_FULL: u16 = 1 << 6;
        let mut bits = POWERUP_SUMMARY | POSITION_HOLD;
        if !self.holdover {
            bits |= LOCKED | REFERENCE_VALID;
        }
        // Derived from how full the log actually is, so clearing it
        // changes the bit.  Answering from a constant would let the
        // daemon's clear-when-full logic loop for ever against a
        // receiver that never stopped saying it was full.
        if self.log_entries >= LOG_ALMOST_FULL_AT {
            bits |= LOG_ALMOST_FULL;
        }
        bits
    }

    /// Nudge the values a little, so nothing reads as a constant.
    ///
    /// Around the base rather than around a fixed number: a receiver
    /// built with a railed EFC has to stay railed, or the fault it was
    /// created to represent disappears on the first command.
    fn drift(&mut self) {
        // A slow ramp on EFC and a faster cycle on the interval, which
        // between them give a trend to plot and a spread to average.
        let phase = (self.ticks % 600) as f64 / 600.0;
        self.efc_raw = self.base.efc_raw.saturating_add((phase * 40.0) as u32);
        self.temperature = self.base.temperature + (self.ticks % 120) as f64 * 0.002;
        self.time_interval = self.base.time_interval + ((self.ticks % 7) as f64 - 3.0) * 1e-9;
    }

    /// IEEE 488.2 common commands, which are not in the dialect table
    /// under the names a caller sends.
    fn common(&mut self, command: &str) -> Option<Answer> {
        Some(match command.to_ascii_uppercase().as_str() {
            "*IDN?" => Answer::line(self.identity.clone()),
            "*CLS" => {
                self.errors.clear();
                Answer::silent()
            }
            "*TST?" | "*ESR?" | "*ESE?" => Answer::line("+0"),
            // The alarm enable register, at the value a 58503A reports
            // from the factory: questionable and operation summaries
            // enabled, command errors not -- which is why a receiver
            // does not alarm on the -230 it returns for a value that
            // does not exist in its current state.
            "*SRE?" => Answer::line(format!("{:+}", ALARM_ENABLE)),
            // The alarm condition register.  Derived from the latched
            // events rather than answered as a constant: it is what
            // the front-panel lamp is showing, and a simulator that
            // always says zero cannot exercise an alarm at all.  Real
            // time and non-destructive, so reading it must not clear
            // the events it summarises.
            "*STB?" => Answer::line(format!("{:+}", self.alarm_bits())),
            _ => return None,
        })
    }

    /// Queue an error and refuse.
    ///
    /// The queue is bounded, as the receiver's is: a client sending
    /// nothing but bad commands must not be able to grow it without
    /// limit.  A full one keeps the errors it already holds, turns its
    /// last entry into -350 and discards everything after, which is
    /// what IEEE 488.2 asks for and keeps the earliest error -- the one
    /// that usually explains the rest -- readable.
    fn reject(&mut self, code: i32, message: &str) -> Answer {
        if self.errors.len() >= MAX_ERRORS {
            if let Some(last) = self.errors.back_mut() {
                *last = (QUEUE_OVERFLOW_CODE, QUEUE_OVERFLOW_MESSAGE.to_owned());
            }
        } else {
            self.errors.push_back((code, message.to_owned()));
        }
        Answer {
            lines: Vec::new(),
            accepted: false,
        }
    }

    fn answer(&mut self, id: CommandId, argument: &str) -> Answer {
        // Through the library's own conversion rather than a second
        // copy of the arithmetic, or the simulator could disagree with
        // the code it exists to test.
        let percent = EfcPercent::from_raw(self.efc_raw).map_or(0.0, |e| e.percent());
        match id {
            CommandId::Idn => Answer::line(self.identity.clone()),
            CommandId::SyncState => Answer::line(if self.holdover { "HOLD" } else { "LOCK" }),
            CommandId::Tfom => Answer::line("+3"),
            CommandId::Ffom => Answer::line("+1"),
            CommandId::Tinterval => Answer::line(real(self.time_interval)),
            CommandId::Efc => Answer::line(real(percent)),
            CommandId::EfcAbsolute => Answer::line(format!("{:+}", self.efc_raw)),
            CommandId::Temperature => Answer::line(real(self.temperature)),
            CommandId::OvenCurrent => Answer::line(real(104.9)),
            CommandId::OvenTempco => Answer::line(real(-33.65)),
            CommandId::HardwareCondition => Answer::line(format!("{:+}", self.hardware)),
            // Reading an event register is what clears it.
            CommandId::OperEvent => {
                Answer::line(format!("{:+}", self.take_event(|e| &mut e.operation)))
            }
            CommandId::QuestEvent => {
                Answer::line(format!("{:+}", self.take_event(|e| &mut e.questionable)))
            }
            CommandId::HardwareEvent => {
                Answer::line(format!("{:+}", self.take_event(|e| &mut e.hardware)))
            }
            CommandId::HoldoverEvent => {
                Answer::line(format!("{:+}", self.take_event(|e| &mut e.holdover)))
            }
            CommandId::PowerupEvent => {
                Answer::line(format!("{:+}", self.take_event(|e| &mut e.powerup)))
            }
            // The factory transition filters this 58503A reports: every
            // condition bit latches on assertion, none on release.
            CommandId::OperPositiveTransition => Answer::line("+127"),
            CommandId::QuestPositiveTransition => Answer::line("+2"),
            CommandId::HardwarePositiveTransition => Answer::line("+5087"),
            CommandId::HoldoverPositiveTransition => Answer::line("+15"),
            CommandId::PowerupPositiveTransition => Answer::line("+7"),
            CommandId::OperNegativeTransition
            | CommandId::QuestNegativeTransition
            | CommandId::HardwareNegativeTransition
            | CommandId::HoldoverNegativeTransition
            | CommandId::PowerupNegativeTransition => Answer::line("+0"),
            CommandId::HoldoverWaiting => Answer::line(if self.holdover { "GPS" } else { "NONE" }),
            CommandId::HoldoverDuration => Answer::line(format!(
                "{},{}",
                real(if self.holdover { 300.0 } else { 0.0 }),
                u8::from(self.holdover)
            )),
            CommandId::HoldoverUncPred => Answer::line(format!("{},0", real(432e-6))),
            // Only meaningful in holdover; the real receiver declines
            // with -230 otherwise, and callers depend on that.
            CommandId::HoldoverUncNow => {
                if self.holdover {
                    Answer::line(real(1.0e-6))
                } else {
                    self.reject(-230, "Data corrupt or stale")
                }
            }
            CommandId::StatusScreen => Answer {
                lines: SCREEN.lines().map(str::to_owned).collect(),
                accepted: true,
            },
            CommandId::StatusScreenLines => Answer::line(format!("{:+}", SCREEN.lines().count())),
            CommandId::PositionAvg | CommandId::PositionActual | CommandId::PositionHoldLast
                if self.no_fix =>
            {
                self.reject(-230, "Data corrupt or stale")
            }
            CommandId::PositionAvg | CommandId::PositionActual | CommandId::PositionHoldLast => {
                Answer::line("N,+37,+22,+3.02770E+001,W,+122,+5,+3.48160E+001,+4.35100E+001")
            }
            CommandId::Date if self.no_fix => self.reject(-230, "Data corrupt or stale"),
            CommandId::Date => {
                let d = rolled_back_date();
                Answer::line(format!("{:+},{:+},{:+}", d.year(), d.month(), d.day()))
            }
            CommandId::Time if self.no_fix => self.reject(-230, "Data corrupt or stale"),
            CommandId::Time => Answer::line("+20,+4,+31"),
            CommandId::LogCount => Answer::line(format!("{:+}", self.log_entries)),
            // The count is a compare-and-swap: the instrument refuses
            // if it does not match, which is what stops an entry
            // written since the reader last looked from being erased
            // unread.
            CommandId::LogClear => {
                let claimed = argument.parse::<i64>().ok();
                if claimed.is_some_and(|c| c != self.log_entries) {
                    self.reject(-222, "Data out of range")
                } else {
                    self.log_entries = 1;
                    self.log_cleared = true;
                    Answer::silent()
                }
            }
            // Without an entry number this is the newest entry, with
            // one it is that entry; the oldest has its own command.
            CommandId::LogRead => {
                let entry = argument.parse::<i64>().unwrap_or(self.log_entries);
                if !(1..=self.log_entries).contains(&entry) {
                    self.reject(-222, "Data out of range")
                } else if self.log_cleared {
                    Answer::line(CLEARED_ENTRY.to_owned())
                } else {
                    let era = if self.log_unchanged == Some(entry) {
                        0
                    } else {
                        self.log_era
                    };
                    Answer::line(log_entry(entry, era))
                }
            }
            CommandId::LogOldest => Answer::line(if self.log_cleared {
                CLEARED_ENTRY.to_owned()
            } else {
                log_entry(1, self.log_era)
            }),
            CommandId::OperCondition => Answer::line(format!("{:+}", self.operation_bits())),
            CommandId::HoldoverCondition => Answer::line(format!("{:+}", u16::from(self.holdover))),
            CommandId::PowerupCondition => Answer::line("+7"),
            CommandId::SatTracking => Answer::line("+3,+4,+16,+26,+28,+31"),
            CommandId::SatTrackingCount => Answer::line("+6"),
            CommandId::SatVisible => Answer::line("+1,+3,+4,+6,+9,+16,+26,+28,+31"),
            CommandId::SatVisibleCount => Answer::line("+9"),
            CommandId::ElevationMask => Answer::line("+10"),
            CommandId::AntennaDelay => Answer::line(real(0.0)),
            CommandId::TimeValid | CommandId::LedGpslock => Answer::line("1"),
            CommandId::LedHoldover => Answer::line(u8::from(self.holdover).to_string()),
            // Lit when the alarm condition and the alarm enable
            // register overlap.  097-59551-02 figure 5-2.
            CommandId::LedAlarm => {
                Answer::line(u8::from(self.alarm_bits() & ALARM_ENABLE != 0).to_string())
            }
            CommandId::LifetimeCount => Answer::line("+36302"),

            // Control commands are accepted and change what they say
            // they change, so a test can see the effect.
            CommandId::HoldoverInitiate => {
                self.holdover = true;
                Answer::silent()
            }
            CommandId::HoldoverRecover => {
                self.holdover = false;
                Answer::silent()
            }
            CommandId::ElevationMaskSet
            | CommandId::AntennaDelaySet
            | CommandId::SurveyOnce
            | CommandId::SyncImmediate
            | CommandId::HoldoverThreshSet => Answer::silent(),

            // Everything else parses but has no answer here.  -221 is
            // what the real receiver gives for a value that does not
            // exist in its current state, which callers already treat
            // as "absent" rather than as a failure.
            _ => self.reject(-221, "Settings conflict"),
        }
    }
}

/// Format as the receiver does: signed scientific notation.
fn real(value: f64) -> String {
    let text = format!("{value:+.5E}");
    // The receiver writes a three-digit exponent with a sign.
    match text.split_once('E') {
        Some((mantissa, exponent)) => {
            let sign = if exponent.starts_with('-') { '-' } else { '+' };
            let digits: String = exponent.chars().filter(char::is_ascii_digit).collect();
            format!("{mantissa}E{sign}{:0>3}", digits)
        }
        None => text,
    }
}

fn split(command: &str) -> (&str, &str) {
    command
        .split_once(char::is_whitespace)
        .map_or((command, ""), |(h, a)| (h, a.trim()))
}

fn eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

#[cfg(test)]
mod tests {
    use super::Receiver;

    /// Reading an event register is what clears it.
    ///
    /// The distinguishing property of an event register, and the one a
    /// simulator answering from the current condition would hide: the
    /// daemon would look correct while re-recording the same transition
    /// on every pass for ever.
    #[test]
    fn an_event_is_returned_once_and_then_gone() {
        let mut receiver = Receiver::default();
        // Bit 0 of the questionable register: the receiver stepped its
        // own clock to match the satellites.
        receiver.raise_event("questionable", 1);

        let first = receiver.respond(":STATus:QUEStionable:EVENt?");
        assert_eq!(first.lines, vec!["+1".to_owned()]);

        let second = receiver.respond(":STATus:QUEStionable:EVENt?");
        assert_eq!(
            second.lines,
            vec!["+0".to_owned()],
            "the read should have cleared it"
        );
    }

    /// Events latch, so two transitions before a read arrive together.
    #[test]
    fn events_accumulate_until_they_are_read() {
        let mut receiver = Receiver::default();
        receiver.raise_event("hardware", 1 << 6);
        receiver.raise_event("hardware", 1 << 7);
        let reply = receiver.respond(":STATus:OPERation:HARDware:EVENt?");
        assert_eq!(reply.lines, vec![format!("+{}", (1 << 6) | (1 << 7))]);
    }

    /// Reading the alarm condition does not clear it, and reading the
    /// events does.
    ///
    /// The distinction the whole design turns on: `*STB?` is how the
    /// daemon watches the alarm without taking it, and an event read is
    /// what would take it.
    #[test]
    fn the_alarm_survives_being_read_and_dies_when_the_events_are_taken() {
        let mut receiver = Receiver::default();
        receiver.raise_event("questionable", 1);

        let alarm = || "+72".to_owned(); // questionable summary + master
        assert_eq!(receiver.respond("*STB?").lines, vec![alarm()]);
        assert_eq!(
            receiver.respond("*STB?").lines,
            vec![alarm()],
            "reading the alarm condition must not clear it"
        );
        assert_eq!(receiver.respond(":LED:ALARm?").lines, vec!["1".to_owned()]);

        // Now take the event, which is what the daemon deliberately
        // does not do.
        receiver.respond(":STATus:QUEStionable:EVENt?");
        assert_eq!(
            receiver.respond("*STB?").lines,
            vec!["+0".to_owned()],
            "clearing the events clears the alarm"
        );
        assert_eq!(receiver.respond(":LED:ALARm?").lines, vec!["0".to_owned()]);
    }

    /// The condition register is not cleared by reading it.
    #[test]
    fn a_condition_survives_being_read() {
        let mut receiver = Receiver::default();
        let first = receiver.respond(":STATus:OPERation:CONDition?");
        let second = receiver.respond(":STATus:OPERation:CONDition?");
        assert_eq!(first.lines, second.lines);
        assert_ne!(first.lines, vec!["+0".to_owned()]);
    }
}
