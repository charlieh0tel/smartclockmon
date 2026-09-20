//! Values a receiver reports, as types rather than strings.

use std::fmt;

use serde::Deserialize;
use serde::Serialize;

/// Time figure of merit: how accurate the 1 PPS is.  Lower is better;
/// the receiver reports 1 through 9.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Tfom(u8);

/// Frequency figure of merit: how stable the 10 MHz is.  Lower is
/// better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Ffom(u8);

/// A GPS satellite, identified by pseudo-random noise code, 1 to 32.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Prn(u8);

macro_rules! bounded_u8 {
    ($t:ty, $lo:expr, $hi:expr, $what:literal) => {
        impl $t {
            /// Construct, rejecting values outside the documented range.
            pub fn new(value: u8) -> Option<Self> {
                ($lo..=$hi).contains(&value).then_some(Self(value))
            }

            /// The underlying value.
            pub fn get(self) -> u8 {
                self.0
            }
        }

        impl fmt::Display for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

bounded_u8!(Tfom, 1, 9, "time figure of merit");
bounded_u8!(Ffom, 0, 9, "frequency figure of merit");
bounded_u8!(Prn, 1, 32, "PRN");

/// Oscillator electronic frequency control, as a percentage of its
/// range.  Drift toward either rail is how an ageing OCXO fails: once
/// the control voltage saturates the receiver can no longer discipline
/// it, which the hardware register reports as bits 6 and 7.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct EfcPercent(f64);

impl EfcPercent {
    /// Construct, rejecting values outside the documented -100 to +100.
    pub fn new(percent: f64) -> Option<Self> {
        (-100.0..=100.0).contains(&percent).then_some(Self(percent))
    }

    /// The percentage.
    pub fn percent(self) -> f64 {
        self.0
    }

    /// How much of the control range is used, 0.0 at centre and 1.0 at
    /// either rail.  This, not the signed percentage, is what says how
    /// close the oscillator is to being untunable.
    pub fn range_used(self) -> f64 {
        self.0.abs() / 100.0
    }

    /// The percentage the receiver would report for a raw EFC value.
    ///
    /// `:DIAGnostic:ROSCillator:EFControl:ABSolute?` returns that value
    /// and it is 20 bits wide: 713392 gives 36.0687, exactly what
    /// `:RELative?` returned at the same moment.  Neither command is
    /// documented; both were found by sweeping the receiver.
    ///
    /// The converter behind it is not 20 bits.  On the Z3801A it is a
    /// 16-bit AD569 whose low-order bits are dithered to interpolate
    /// the remaining four, which Tom Van Baak traced with a scope at
    /// <http://www.leapsecond.com/pages/z3801a-efc/>.  The arithmetic
    /// here is unaffected, but "20-bit DAC" would be wrong.
    pub fn from_raw(value: u32) -> Option<Self> {
        Self::new(f64::from(value) / f64::from(EFC_FULL_SCALE) * 200.0 - 100.0)
    }
}

/// The span of the raw EFC value.  Twenty bits.
const EFC_FULL_SCALE: u32 = 1 << 20;

impl fmt::Display for EfcPercent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:+.1}%", self.0)
    }
}

/// Which mode the disciplining loop is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SmartClockMode {
    /// Locked to GPS.
    Locked,
    /// Recovering from holdover.
    Recovery,
    /// In holdover, manually or otherwise.
    Holdover,
    /// Waiting for conditions that allow recovery.
    Waiting,
    /// Powering up, before the first lock.
    PowerUp,
    /// Diagnostic or temporary start-up mode.
    Other,
}

impl SmartClockMode {
    /// Parse the word `:SYNChronization:STATe?` returns.
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_ascii_uppercase().as_str() {
            "LOCK" => Some(Self::Locked),
            "REC" => Some(Self::Recovery),
            "HOLD" => Some(Self::Holdover),
            "WAIT" => Some(Self::Waiting),
            "POW" => Some(Self::PowerUp),
            "OTH" => Some(Self::Other),
            _ => None,
        }
    }
}

/// Why the receiver has not left holdover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HoldoverWaitReason {
    /// An internal hardware fault.
    Hardware,
    /// No satellites.
    Gps,
    /// The interval between GPS and internal 1 PPS exceeds the limit.
    Limit,
    /// Not waiting.
    None,
}

impl HoldoverWaitReason {
    /// Parse the word `:SYNChronization:HOLDover:WAITing?` returns.
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_ascii_uppercase().as_str() {
            "HARD" | "HARDWARE" => Some(Self::Hardware),
            "GPS" => Some(Self::Gps),
            "LIM" | "LIMIT" => Some(Self::Limit),
            "NONE" => Some(Self::None),
            _ => None,
        }
    }
}

/// Bits of `:STATus:OPERation:HARDware:CONDition?`.
///
/// Bits 6 and 7 are the ones that matter for an ageing oscillator: they
/// say the control voltage has reached, or nearly reached, the end of
/// its range.  Bit 5 is unused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareCondition(u16);

/// One named bit of the hardware condition register.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HardwareFault {
    /// Bit 0.
    SelfTest,
    /// Bit 1.
    Supply15VPositive,
    /// Bit 2.
    Supply15VNegative,
    /// Bit 3.
    Supply5V,
    /// Bit 4.
    OvenSupply,
    /// Bit 6.  The oscillator is close to untunable.
    EfcNearFullScale,
    /// Bit 7.  The oscillator can no longer be disciplined.
    EfcFullScale,
    /// Bit 8.
    Gps1PpsFailure,
    /// Bit 9.
    GpsFailure,
    /// Bit 10.
    TimeIntervalFailed,
    /// Bit 11.
    EepromWriteFailed,
    /// Bit 12.
    InternalReferenceFailure,
}

/// Every fault with its bit position, in register order.
const HARDWARE_FAULTS: [(u16, HardwareFault); 12] = [
    (0, HardwareFault::SelfTest),
    (1, HardwareFault::Supply15VPositive),
    (2, HardwareFault::Supply15VNegative),
    (3, HardwareFault::Supply5V),
    (4, HardwareFault::OvenSupply),
    (6, HardwareFault::EfcNearFullScale),
    (7, HardwareFault::EfcFullScale),
    (8, HardwareFault::Gps1PpsFailure),
    (9, HardwareFault::GpsFailure),
    (10, HardwareFault::TimeIntervalFailed),
    (11, HardwareFault::EepromWriteFailed),
    (12, HardwareFault::InternalReferenceFailure),
];

impl HardwareCondition {
    /// Wrap a raw register value.
    pub fn from_bits(bits: u16) -> Self {
        Self(bits)
    }

    /// The raw register value.
    pub fn bits(self) -> u16 {
        self.0
    }

    /// Whether any fault is asserted.
    pub fn is_healthy(self) -> bool {
        self.faults().next().is_none()
    }

    /// The faults currently asserted.
    pub fn faults(self) -> impl Iterator<Item = HardwareFault> {
        HARDWARE_FAULTS
            .into_iter()
            .filter(move |(bit, _)| self.0 & (1 << bit) != 0)
            .map(|(_, fault)| fault)
    }
}

impl HardwareFault {
    /// A short description for display.
    pub fn describe(self) -> &'static str {
        match self {
            Self::SelfTest => "selftest failure",
            Self::Supply15VPositive => "+15V supply out of tolerance",
            Self::Supply15VNegative => "-15V supply out of tolerance",
            Self::Supply5V => "+5V supply out of tolerance",
            Self::OvenSupply => "oven supply out of tolerance",
            Self::EfcNearFullScale => "EFC near full scale",
            Self::EfcFullScale => "EFC at full scale",
            Self::Gps1PpsFailure => "GPS 1 PPS failure",
            Self::GpsFailure => "GPS failure",
            Self::TimeIntervalFailed => "time interval measurement failed",
            Self::EepromWriteFailed => "EEPROM write failed",
            Self::InternalReferenceFailure => "internal reference failure",
        }
    }
}

/// Bits of `:STATus:OPERation:HOLDover:CONDition?`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldoverCondition(u16);

impl HoldoverCondition {
    /// Wrap a raw register value.
    pub fn from_bits(bits: u16) -> Self {
        Self(bits)
    }

    /// The raw register value.
    pub fn bits(self) -> u16 {
        self.0
    }

    /// Bit 0: in holdover.
    pub fn holding(self) -> bool {
        self.0 & 1 != 0
    }

    /// Bit 1: waiting to recover.
    pub fn waiting_to_recover(self) -> bool {
        self.0 & (1 << 1) != 0
    }

    /// Bit 2: recovering.
    pub fn recovering(self) -> bool {
        self.0 & (1 << 2) != 0
    }

    /// Bit 3: holdover has run past its duration threshold.
    pub fn exceeding_threshold(self) -> bool {
        self.0 & (1 << 3) != 0
    }
}

/// Which vertical datum a height is referenced to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Datum {
    /// Height above the GPS ellipsoid.
    Ellipsoid,
    /// Height above mean sea level.
    MeanSeaLevel,
}

impl fmt::Display for Datum {
    /// Prints as the receiver's own status screen labels it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ellipsoid => write!(f, "GPS"),
            Self::MeanSeaLevel => write!(f, "MSL"),
        }
    }
}

/// An antenna position.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Position {
    /// Degrees north, negative for south.
    pub latitude: f64,
    /// Degrees east, negative for west.
    pub longitude: f64,
    /// Height in metres.
    pub height: f64,
    /// What the height is referenced to.  The 58503A reports mean sea
    /// level, the 58503B the ellipsoid.
    pub datum: Datum,
}

/// One satellite in the status screen's table.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SatelliteInfo {
    /// Which satellite.
    pub prn: Prn,
    /// Above the horizon, absent while acquiring.
    pub elevation: Option<Degrees>,
    /// Clockwise from north, absent while acquiring.
    pub azimuth: Option<Degrees>,
    /// Signal strength as the screen reports it.
    pub signal: Option<SignalStrength>,
    /// Whether the receiver is using this satellite.  Taken from which
    /// column group the row sits in, not from the asterisk: a satellite
    /// can be untracked without the receiver attempting it.
    pub tracked: bool,
    /// Whether the receiver is attempting to acquire it, shown as a
    /// leading asterisk.  Only meaningful when not tracked.
    pub acquiring: bool,
}

/// Whether a leap second is coming, and which way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LeapPending {
    /// No adjustment pending.
    None,
    /// A second will be inserted.
    Positive,
    /// A second will be removed.
    Negative,
}

/// A time quantity in seconds, as every receiver reply expresses them.
///
/// Wrapped because the same field is read in three scales depending on
/// what it is: the 1 PPS interval is natural in nanoseconds, holdover
/// uncertainty in microseconds, holdover duration in seconds.  Callers
/// that hand-multiply by `1e9` and `1e6` get it wrong eventually.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Seconds(f64);

impl Seconds {
    /// Wrap a count of seconds.
    pub fn new(seconds: f64) -> Self {
        Self(seconds)
    }

    /// As seconds.
    pub fn as_secs(self) -> f64 {
        self.0
    }

    /// As milliseconds.
    pub fn as_millis(self) -> f64 {
        self.0 * 1e3
    }

    /// As microseconds.
    pub fn as_micros(self) -> f64 {
        self.0 * 1e6
    }

    /// As nanoseconds.
    pub fn as_nanos(self) -> f64 {
        self.0 * 1e9
    }
}

impl fmt::Display for Seconds {
    /// Print in whichever scale keeps the number readable, which is how
    /// the receiver's own status screen renders these.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let magnitude = self.0.abs();
        if magnitude == 0.0 {
            write!(f, "0 s")
        } else if magnitude < 1e-6 {
            write!(f, "{:.1} ns", self.as_nanos())
        } else if magnitude < 1e-3 {
            write!(f, "{:.1} us", self.as_micros())
        } else if magnitude < 1.0 {
            write!(f, "{:.1} ms", self.as_millis())
        } else {
            write!(f, "{:.1} s", self.0)
        }
    }
}

/// How long holdover has run, and whether it is running now.
///
/// The receiver returns both from one query, and the pair means
/// different things depending on the flag: elapsed so far while the
/// flag is set, the length of the last holdover while it is clear.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HoldoverDuration {
    /// Time in holdover.
    pub elapsed: Seconds,
    /// Whether the receiver is in holdover now.
    pub active: bool,
}

impl HoldoverDuration {
    /// Build from the value and flag the receiver returns.
    pub fn new(elapsed: Seconds, active: bool) -> Self {
        Self { elapsed, active }
    }
}

/// A time of day as the receiver reports it.
///
/// `second` may be 60: the receiver represents a positive leap second
/// that way, so this cannot be a `jiff::civil::Time`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TimeOfDay {
    /// Hour, 0 to 23.
    pub hour: u8,
    /// Minute, 0 to 59.
    pub minute: u8,
    /// Second, 0 to 60.
    pub second: u8,
}

impl TimeOfDay {
    /// Construct, rejecting anything outside the receiver's range.
    pub fn new(hour: u8, minute: u8, second: u8) -> Option<Self> {
        (hour < 24 && minute < 60 && second <= 60).then_some(Self {
            hour,
            minute,
            second,
        })
    }

    /// Whether this is the leap second itself.
    pub fn is_leap_second(self) -> bool {
        self.second == 60
    }
}

impl fmt::Display for TimeOfDay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}:{:02}", self.hour, self.minute, self.second)
    }
}

/// An offset from UTC, as `:PTIMe:TZONe` expresses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UtcOffset {
    /// Whole hours, signed.
    pub hours: i8,
    /// Minutes, signed the same way as the hours.
    pub minutes: i8,
}

impl UtcOffset {
    /// Wrap an hours and minutes pair.
    pub fn new(hours: i8, minutes: i8) -> Self {
        Self { hours, minutes }
    }

    /// Whether this is UTC itself.
    pub fn is_utc(self) -> bool {
        self.hours == 0 && self.minutes == 0
    }
}

impl fmt::Display for UtcOffset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.hours < 0 || self.minutes < 0 {
            '-'
        } else {
            '+'
        };
        write!(
            f,
            "{sign}{:02}:{:02}",
            self.hours.unsigned_abs(),
            self.minutes.unsigned_abs()
        )
    }
}

/// An entry from the receiver's error queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorEntry {
    /// SCPI error code.  Negative codes are standard, positive ones are
    /// device specific, and zero means the queue is empty.
    pub code: i32,
    /// The quoted description.
    pub message: String,
}

impl ErrorEntry {
    /// Whether this entry means "no error".
    pub fn is_empty(&self) -> bool {
        self.code == 0
    }

    /// Whether the receiver declined because of its current state
    /// rather than because the command was wrong.
    ///
    /// -221 is a settings conflict and -230 is stale data.  Both mean
    /// the header parsed and the value simply does not exist right now,
    /// such as present holdover error while locked.
    pub fn is_state_refusal(&self) -> bool {
        self.code == -221 || self.code == -230
    }
}

/// An angle in whole degrees, as the satellite table reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Degrees(i16);

impl Degrees {
    /// Wrap an angle.
    pub fn new(degrees: i16) -> Self {
        Self(degrees)
    }

    /// The angle.
    pub fn get(self) -> i16 {
        self.0
    }
}

impl fmt::Display for Degrees {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} deg", self.0)
    }
}

/// Signal strength as the status screen reports it.
///
/// Deliberately opaque.  Firmware 3704-C heads this column `SS` over
/// roughly 28 to 114 while the manuals head it `C/N` over roughly 36 to
/// 49, and neither documents a unit.  Treating it as a bare number
/// invites comparing values that are not on the same scale, so it is
/// only ever compared with others from the same receiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SignalStrength(i16);

impl SignalStrength {
    /// Wrap a reading.
    pub fn new(value: i16) -> Self {
        Self(value)
    }

    /// The raw number, in whatever units this firmware uses.
    pub fn raw(self) -> i16 {
        self.0
    }
}

impl fmt::Display for SignalStrength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A serial line rate the receiver supports.
///
/// An enum rather than a number because the receiver accepts only these
/// four.  Opening the port at an unsupported rate does not fail
/// loudly -- it just produces garbage, which looks like a dead
/// receiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum BaudRate {
    /// 1200 baud.
    B1200,
    /// 2400 baud.
    B2400,
    /// 9600 baud, the factory default.
    #[default]
    B9600,
    /// 19200 baud, the fastest the receiver offers.
    B19200,
}

impl BaudRate {
    /// Every rate the receiver accepts.
    pub const ALL: [BaudRate; 4] = [Self::B1200, Self::B2400, Self::B9600, Self::B19200];

    /// Recognise a rate, rejecting any the receiver cannot use.
    pub fn new(rate: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.get() == rate)
    }

    /// The rate in bits per second.
    pub fn get(self) -> u32 {
        match self {
            Self::B1200 => 1200,
            Self::B2400 => 2400,
            Self::B9600 => 9600,
            Self::B19200 => 19200,
        }
    }

    /// Roughly how long `bytes` take on the wire at this rate, at the
    /// receiver's 8N1 framing of ten bits per byte.
    ///
    /// This is what makes the poll tiers a budget rather than a
    /// preference: the status screen is about 1.8 KB, close to a second
    /// at 19200.
    pub fn wire_time(self, bytes: u32) -> std::time::Duration {
        std::time::Duration::from_secs_f64(f64::from(bytes) * 10.0 / f64::from(self.get()))
    }
}

impl fmt::Display for BaudRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.get())
    }
}

#[cfg(test)]
mod tests {
    use super::BaudRate;
    use super::Seconds;
    use super::TimeOfDay;
    use super::UtcOffset;

    #[test]
    fn a_time_quantity_picks_a_readable_scale() {
        assert_eq!(Seconds::new(-4.1e-9).to_string(), "-4.1 ns");
        assert_eq!(Seconds::new(432e-6).to_string(), "432.0 us");
        assert_eq!(Seconds::new(1.5e-3).to_string(), "1.5 ms");
        assert_eq!(Seconds::new(86400.0).to_string(), "86400.0 s");
        assert_eq!(Seconds::new(0.0).to_string(), "0 s");
    }

    #[test]
    fn scale_conversions_agree() {
        let value = Seconds::new(1e-6);
        assert!((value.as_nanos() - 1000.0).abs() < 1e-9);
        assert!((value.as_micros() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn only_the_four_supported_rates_are_accepted() {
        // 38400 is a rate the receiver cannot use; opening at it yields
        // garbage rather than an error, so it must be refused here.
        assert_eq!(BaudRate::new(19200), Some(BaudRate::B19200));
        assert_eq!(BaudRate::new(9600), Some(BaudRate::B9600));
        assert_eq!(BaudRate::new(38400), None);
        assert_eq!(BaudRate::new(115200), None);
        assert_eq!(BaudRate::default(), BaudRate::B9600);
    }

    #[test]
    fn wire_time_makes_the_poll_budget_concrete() {
        // The status screen is about 1.8 KB, which is most of a second
        // at 19200 and why it cannot share the one-second tier.
        let screen = BaudRate::B19200.wire_time(1800);
        assert!(screen.as_secs_f64() > 0.9, "{screen:?}");
        assert!(screen.as_secs_f64() < 1.0, "{screen:?}");
        // The same screen at the factory default is twice as bad again.
        assert!(BaudRate::B9600.wire_time(1800).as_secs_f64() > 1.8);
    }

    #[test]
    fn a_leap_second_is_representable_but_a_bad_time_is_not() {
        assert!(TimeOfDay::new(23, 59, 60).expect("leap").is_leap_second());
        assert!(TimeOfDay::new(24, 0, 0).is_none());
        assert!(TimeOfDay::new(0, 60, 0).is_none());
        assert!(TimeOfDay::new(0, 0, 61).is_none());
    }

    #[test]
    fn an_offset_prints_with_one_sign_for_the_pair() {
        assert_eq!(UtcOffset::new(0, 0).to_string(), "+00:00");
        assert_eq!(UtcOffset::new(-8, 0).to_string(), "-08:00");
        assert_eq!(UtcOffset::new(5, 30).to_string(), "+05:30");
        assert!(UtcOffset::default().is_utc());
    }
}

#[cfg(test)]
mod efc_tests {
    use super::EfcPercent;

    #[test]
    fn the_raw_value_converts_to_the_percentage_the_receiver_reports() {
        // Read from a 58503A moments apart: ABSolute? gave 713392 and
        // RELative? gave +3.60687E+001.
        let converted = EfcPercent::from_raw(713392).expect("in range");
        assert!(
            (converted.percent() - 36.0687).abs() < 1e-4,
            "got {converted}"
        );
    }

    #[test]
    fn the_ends_and_the_middle_land_where_they_should() {
        assert_eq!(EfcPercent::from_raw(0).expect("low").percent(), -100.0);
        assert_eq!(EfcPercent::from_raw(1 << 19).expect("mid").percent(), 0.0);
        // One below full scale, since 2^20 itself would exceed +100.
        let high = EfcPercent::from_raw((1 << 20) - 1).expect("high");
        assert!(high.percent() < 100.0 && high.percent() > 99.999);
    }
}
