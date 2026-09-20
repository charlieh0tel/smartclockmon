//! Values a receiver reports, as types rather than strings.

use std::fmt;

/// Time figure of merit: how accurate the 1 PPS is.  Lower is better;
/// the receiver reports 1 through 9.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Tfom(u8);

/// Frequency figure of merit: how stable the 10 MHz is.  Lower is
/// better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ffom(u8);

/// A GPS satellite, identified by pseudo-random noise code, 1 to 32.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
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
}

impl fmt::Display for EfcPercent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:+.1}%", self.0)
    }
}

/// Which mode the disciplining loop is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardwareCondition(u16);

/// One named bit of the hardware condition register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Datum {
    /// Height above the GPS ellipsoid.
    Ellipsoid,
    /// Height above mean sea level.
    MeanSeaLevel,
}

/// An antenna position.
#[derive(Debug, Clone, Copy, PartialEq)]
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
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SatelliteInfo {
    /// Which satellite.
    pub prn: Prn,
    /// Degrees above the horizon, absent while acquiring.
    pub elevation: Option<i16>,
    /// Degrees clockwise from north, absent while acquiring.
    pub azimuth: Option<i16>,
    /// Signal strength as the screen reports it.  Firmware 3704-C heads
    /// this column `SS` over roughly 28 to 114; the manuals head it
    /// `C/N` over roughly 36 to 49.  The units are not documented, so
    /// this is the raw number.
    pub signal: Option<i16>,
    /// Whether the receiver is using this satellite.  Taken from which
    /// column group the row sits in, not from the asterisk: a satellite
    /// can be untracked without the receiver attempting it.
    pub tracked: bool,
    /// Whether the receiver is attempting to acquire it, shown as a
    /// leading asterisk.  Only meaningful when not tracked.
    pub acquiring: bool,
}

/// Whether a leap second is coming, and which way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeapPending {
    /// No adjustment pending.
    None,
    /// A second will be inserted.
    Positive,
    /// A second will be removed.
    Negative,
}
