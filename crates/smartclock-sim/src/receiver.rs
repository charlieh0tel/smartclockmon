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

/// How many errors the queue holds.  Public so a test can state the
/// bound it expects rather than repeating the number.
pub const MAX_ERRORS: usize = 32;

/// SCPI's "Queue overflow", which a full queue reports in place of its
/// last entry.
const QUEUE_OVERFLOW_CODE: i32 = -350;
const QUEUE_OVERFLOW_MESSAGE: &str = "Queue overflow";

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
            identity: "HEWLETT-PACKARD,58503A,3710A01056,3704-C".to_owned(),
            efc_raw: 713_587,
            temperature: 37.40,
            time_interval: -4.8e-9,
            hardware: 0,
            holdover: false,
            dialect: Dialect::Hp58503,
            ticks: 0,
            base: Base {
                efc_raw: 713_587,
                temperature: 37.40,
                time_interval: -4.8e-9,
            },
            errors: VecDeque::new(),
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

    /// Answer one command.
    pub fn respond(&mut self, command: &str) -> Answer {
        self.ticks += 1;
        self.drift();

        let command = command.trim();
        if command.is_empty() {
            return Answer::silent();
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

        let (header, _argument) = split(command);
        let found = self
            .dialect
            .specs()
            .iter()
            .find(|s| eq(s.scpi, command) || eq(s.scpi, header));
        match found {
            Some(spec) => self.answer(spec.id),
            None => self.reject(-113, "Undefined header"),
        }
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
            "*TST?" | "*ESR?" | "*ESE?" | "*SRE?" | "*STB?" => Answer::line("+0"),
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

    fn answer(&mut self, id: CommandId) -> Answer {
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
            CommandId::HardwareCondition | CommandId::HardwareEvent => {
                Answer::line(format!("{:+}", self.hardware))
            }
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
            CommandId::PositionAvg | CommandId::PositionActual | CommandId::PositionHoldLast => {
                Answer::line("N,+37,+22,+3.02770E+001,W,+122,+5,+3.48160E+001,+4.35100E+001")
            }
            CommandId::Date => Answer::line("+2007,+2,+4"),
            CommandId::Time => Answer::line("+20,+4,+31"),
            CommandId::LogCount => Answer::line("+222"),
            CommandId::LogRead | CommandId::LogOldest => {
                Answer::line("\"Log 222:20050727.06:17:34: Holdover started, not tracking GPS\"")
            }
            CommandId::SatTracking => Answer::line("+3,+4,+16,+26,+28,+31"),
            CommandId::SatTrackingCount => Answer::line("+6"),
            CommandId::SatVisible => Answer::line("+1,+3,+4,+6,+9,+16,+26,+28,+31"),
            CommandId::SatVisibleCount => Answer::line("+9"),
            CommandId::ElevationMask => Answer::line("+10"),
            CommandId::AntennaDelay => Answer::line(real(0.0)),
            CommandId::TimeValid | CommandId::LedGpslock => Answer::line("1"),
            CommandId::LedHoldover => Answer::line(u8::from(self.holdover).to_string()),
            CommandId::LedAlarm => Answer::line("0"),
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
