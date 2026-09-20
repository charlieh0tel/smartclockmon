//! Parsers for the response formats in `097-59551-02` chapter 5.
//!
//! The formats are written in the manual as patterns like `±dd` and
//! `±d.dEe`.  Every parser here is exercised against replies actually
//! recorded from a 58503A, because the manual's rendering and the
//! wire's are not always the same thing.

use jiff::civil::Date;
use jiff::civil::date;

use crate::error::Error;
use crate::error::Result;
use crate::types::Datum;
use crate::types::EfcPercent;
use crate::types::ErrorEntry;
use crate::types::Ffom;
use crate::types::HoldoverDuration;
use crate::types::LeapPending;
use crate::types::Position;
use crate::types::Prn;
use crate::types::Seconds;
use crate::types::Tfom;
use crate::types::TimeOfDay;
use crate::types::UtcOffset;

fn bad(reply: &str, expected: &'static str) -> Error {
    Error::Parse {
        reply: reply.to_owned(),
        expected,
    }
}

/// `±dd`: a signed integer, leading `+` and all.
pub fn int(reply: &str) -> Result<i64> {
    let text = reply.trim();
    text.strip_prefix('+')
        .unwrap_or(text)
        .parse()
        .map_err(|_| bad(reply, "an integer"))
}

/// `±d.dEe`: a signed real in scientific notation.  Rust's float parser
/// accepts the receiver's `+3.60971E+001` as written.
pub fn real(reply: &str) -> Result<f64> {
    reply
        .trim()
        .parse()
        .map_err(|_| bad(reply, "a real number"))
}

/// `0 or 1`.
pub fn bool01(reply: &str) -> Result<bool> {
    match reply.trim() {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(bad(reply, "0 or 1")),
    }
}

/// An unquoted word, such as `LOCK` or `NONE`.
pub fn word(reply: &str) -> Result<&str> {
    let text = reply.trim();
    if text.is_empty() {
        return Err(bad(reply, "a word"));
    }
    Ok(text)
}

/// `"XYZ"`: a quoted string.  The manuals print curly quotes; the
/// receiver sends straight ones.
pub fn string(reply: &str) -> Result<&str> {
    Ok(reply
        .trim()
        .trim_matches(|c| c == '"' || c == '\u{201c}' || c == '\u{201d}'))
}

/// `±dd, ...`: a comma-separated integer list.  A lone `0` means the
/// list is empty, which is how the receiver says "none".
pub fn int_list(reply: &str) -> Result<Vec<i64>> {
    let values = reply
        .trim()
        .split(',')
        .map(int)
        .collect::<Result<Vec<_>>>()?;
    Ok(match values.as_slice() {
        [0] => Vec::new(),
        _ => values,
    })
}

/// A list of satellite numbers, rejecting any outside 1 to 32.
pub fn prn_list(reply: &str) -> Result<Vec<Prn>> {
    int_list(reply)?
        .into_iter()
        .map(|n| {
            u8::try_from(n)
                .ok()
                .and_then(Prn::new)
                .ok_or_else(|| bad(reply, "PRNs in 1..=32"))
        })
        .collect()
}

/// `±d.dEe, 0 or 1`: a duration paired with a flag, as holdover
/// duration and predicted uncertainty both return.
pub fn holdover_duration(reply: &str) -> Result<HoldoverDuration> {
    let (value, flag) = reply
        .trim()
        .split_once(',')
        .ok_or_else(|| bad(reply, "a duration and a flag"))?;
    Ok(HoldoverDuration::new(seconds(value)?, bool01(flag)?))
}

/// A `±d.dEe` reply that is a time quantity.
pub fn seconds(reply: &str) -> Result<Seconds> {
    Ok(Seconds::new(real(reply)?))
}

/// `±dd, "XYZ"`: an error queue entry.
pub fn error_entry(reply: &str) -> Result<ErrorEntry> {
    let (code, message) = reply
        .trim()
        .split_once(',')
        .ok_or_else(|| bad(reply, "a code and a message"))?;
    let code = i32::try_from(int(code)?).map_err(|_| bad(reply, "an error code"))?;
    Ok(ErrorEntry {
        code,
        message: string(message)?.to_owned(),
    })
}

/// `±dd, ±dd, ±dd` as a calendar date.
pub fn ymd(reply: &str) -> Result<Date> {
    let parts = reply
        .trim()
        .split(',')
        .map(int)
        .collect::<Result<Vec<_>>>()?;
    let [year, month, day] = parts.as_slice() else {
        return Err(bad(reply, "year, month, day"));
    };
    build_date(*year, *month, *day).ok_or_else(|| bad(reply, "a valid date"))
}

fn build_date(year: i64, month: i64, day: i64) -> Option<Date> {
    let year = i16::try_from(year).ok()?;
    let month = i8::try_from(month).ok()?;
    let day = i8::try_from(day).ok()?;
    Date::new(year, month, day).ok()
}

/// `±dd, ±dd, ±dd` as a time of day.
pub fn hms(reply: &str) -> Result<TimeOfDay> {
    let parts = reply
        .trim()
        .split(',')
        .map(int)
        .collect::<Result<Vec<_>>>()?;
    let [h, m, s] = parts.as_slice() else {
        return Err(bad(reply, "hour, minute, second"));
    };
    match (u8::try_from(*h), u8::try_from(*m), u8::try_from(*s)) {
        (Ok(h), Ok(m), Ok(s)) => TimeOfDay::new(h, m, s).ok_or_else(|| bad(reply, "a valid time")),
        _ => Err(bad(reply, "a valid time")),
    }
}

/// `±dd, ±dd`: a time zone offset in hours and minutes.
pub fn tzone(reply: &str) -> Result<UtcOffset> {
    let parts = reply
        .trim()
        .split(',')
        .map(int)
        .collect::<Result<Vec<_>>>()?;
    let [h, m] = parts.as_slice() else {
        return Err(bad(reply, "hours and minutes"));
    };
    match (i8::try_from(*h), i8::try_from(*m)) {
        (Ok(h), Ok(m)) => Ok(UtcOffset::new(h, m)),
        _ => Err(bad(reply, "an offset in range")),
    }
}

/// A position: hemisphere, degrees, minutes, seconds for each axis,
/// then height.
pub fn position(reply: &str, datum: Datum) -> Result<Position> {
    let f: Vec<&str> = reply.trim().split(',').map(str::trim).collect();
    let [ns, lat_d, lat_m, lat_s, ew, lon_d, lon_m, lon_s, height] = f.as_slice() else {
        return Err(bad(reply, "a position"));
    };
    let latitude =
        signed_dms(ns, "N", "S", lat_d, lat_m, lat_s).ok_or_else(|| bad(reply, "a latitude"))?;
    let longitude =
        signed_dms(ew, "E", "W", lon_d, lon_m, lon_s).ok_or_else(|| bad(reply, "a longitude"))?;
    Ok(Position {
        latitude,
        longitude,
        height: real(height)?,
        datum,
    })
}

fn signed_dms(
    hemisphere: &str,
    positive: &str,
    negative: &str,
    degrees: &str,
    minutes: &str,
    seconds: &str,
) -> Option<f64> {
    let sign = match hemisphere.trim().to_ascii_uppercase() {
        h if h == positive => 1.0,
        h if h == negative => -1.0,
        _ => return None,
    };
    let d = real(degrees).ok()?;
    let m = real(minutes).ok()?;
    let s = real(seconds).ok()?;
    Some(sign * (d + m / 60.0 + s / 3600.0))
}

/// `*IDN?`: manufacturer, model, serial, firmware.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// Manufacturer, `HEWLETT-PACKARD` on the development unit.
    pub manufacturer: String,
    /// Model, such as `58503A`.
    pub model: String,
    /// Serial number.
    pub serial: String,
    /// Firmware revision, such as `3704-C`.
    pub firmware: String,
}

/// Parse `*IDN?`.
pub fn identity(reply: &str) -> Result<Identity> {
    let f: Vec<&str> = reply.trim().split(',').map(str::trim).collect();
    let [manufacturer, model, serial, firmware] = f.as_slice() else {
        return Err(bad(reply, "four comma-separated identity fields"));
    };
    Ok(Identity {
        manufacturer: (*manufacturer).to_owned(),
        model: (*model).to_owned(),
        serial: (*serial).to_owned(),
        firmware: (*firmware).to_owned(),
    })
}

/// A decoded `:PTIMe:TCODe?` message.
#[derive(Debug, Clone, PartialEq)]
pub struct TimeCode {
    /// Date of the next 1 PPS, in T2 format only.  T1 carries seconds
    /// since the GPS epoch instead.
    pub date: Option<Date>,
    /// Time of the next 1 PPS, in T2 format only.
    pub time: Option<TimeOfDay>,
    /// Seconds since 1980-01-06, in T1 format only.
    pub gps_seconds: Option<u32>,
    /// Time figure of merit.
    pub tfom: Tfom,
    /// Frequency figure of merit.
    pub ffom: Ffom,
    /// Whether a leap second is pending.
    pub leap: LeapPending,
    /// The request-for-service summary bit.
    pub service_requested: bool,
    /// Whether the time in this message is valid.
    pub valid: bool,
}

/// Parse `:PTIMe:TCODe?`, verifying the checksum.
///
/// The checksum is the low byte of the sum of every preceding
/// character.  Verifying it is worthwhile: this message is the one
/// reply the receiver emits on a deadline, just before the on-time
/// edge, so a corrupted one is exactly the sort that must not be
/// believed.
pub fn timecode(reply: &str) -> Result<TimeCode> {
    let text = reply.trim();
    let (body, checksum) = text
        .split_at_checked(text.len().checked_sub(2).unwrap_or(text.len()))
        .ok_or_else(|| bad(reply, "a time code"))?;
    let stated = u8::from_str_radix(checksum, 16).map_err(|_| bad(reply, "a checksum"))?;
    let computed = body.bytes().fold(0u8, |acc, b| acc.wrapping_add(b));
    if stated != computed {
        return Err(bad(reply, "a time code whose checksum matches"));
    }

    // Both formats end with the same five status characters.
    let (header, status) = body
        .split_at_checked(
            body.len()
                .checked_sub(5)
                .ok_or_else(|| bad(reply, "a time code"))?,
        )
        .ok_or_else(|| bad(reply, "a time code"))?;
    let status: Vec<char> = status.chars().collect();
    let [t, f, l, r, v] = status.as_slice() else {
        return Err(bad(reply, "five status characters"));
    };

    let digit = |c: &char| c.to_digit(10).and_then(|d| u8::try_from(d).ok());
    let tfom = digit(t)
        .and_then(Tfom::new)
        .ok_or_else(|| bad(reply, "a TFOM"))?;
    let ffom = digit(f)
        .and_then(Ffom::new)
        .ok_or_else(|| bad(reply, "an FFOM"))?;
    let leap = match l {
        '0' => LeapPending::None,
        '+' => LeapPending::Positive,
        '-' => LeapPending::Negative,
        _ => return Err(bad(reply, "a leap second indicator")),
    };

    let mut code = TimeCode {
        date: None,
        time: None,
        gps_seconds: None,
        tfom,
        ffom,
        leap,
        service_requested: *r != '0',
        valid: *v == '0',
    };

    match header.get(..2) {
        Some("T2") => {
            let rest = header.get(2..).unwrap_or_default();
            if rest.len() != 14 {
                return Err(bad(reply, "T2 with 8 date and 6 time digits"));
            }
            let num = |r: std::ops::Range<usize>| -> Option<i64> { rest.get(r)?.parse().ok() };
            let (y, mo, d) = (num(0..4), num(4..6), num(6..8));
            let (h, mi, s) = (num(8..10), num(10..12), num(12..14));
            let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(s)) = (y, mo, d, h, mi, s)
            else {
                return Err(bad(reply, "T2 digits"));
            };
            code.date = Some(build_date(y, mo, d).ok_or_else(|| bad(reply, "a valid T2 date"))?);
            let time = (
                u8::try_from(h).ok(),
                u8::try_from(mi).ok(),
                u8::try_from(s).ok(),
            );
            let (Some(h), Some(mi), Some(s)) = time else {
                return Err(bad(reply, "a valid T2 time"));
            };
            code.time =
                Some(TimeOfDay::new(h, mi, s).ok_or_else(|| bad(reply, "a valid T2 time"))?);
        }
        Some("T1") => {
            let rest = header
                .get(2..)
                .and_then(|r| r.strip_prefix("#H"))
                .ok_or_else(|| bad(reply, "T1 with a #H prefix"))?;
            code.gps_seconds =
                Some(u32::from_str_radix(rest, 16).map_err(|_| bad(reply, "T1 hex seconds"))?);
        }
        _ => return Err(bad(reply, "a T1 or T2 header")),
    }
    Ok(code)
}

/// Turn the EFC reply into a percentage.
pub fn efc(reply: &str) -> Result<EfcPercent> {
    EfcPercent::new(real(reply)?).ok_or_else(|| bad(reply, "a percentage in -100..=100"))
}

/// The epoch GPS time is counted from, for T1 messages.
pub fn gps_epoch() -> Date {
    date(1980, 1, 6)
}
