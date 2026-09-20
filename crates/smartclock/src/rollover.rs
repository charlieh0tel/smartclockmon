//! GPS week number rollover.
//!
//! GPS broadcasts its week number in ten bits, so it wraps every 1024
//! weeks, or 7168 days.  Receivers built before a wrap have to guess
//! which epoch they are in, and firmware that predates one guesses
//! wrong ever after: it reports a date exactly 1024 weeks, or a multiple
//! of that, in the past.
//!
//! The development unit does this.  Firmware 3704-C, from 2005, missed
//! the April 2019 wrap and reports 2007-02-04 for 2026-09-20 -- exactly
//! 7168 days.  Its time of day, its 1 PPS and its 10 MHz are unaffected,
//! and its GPS-UTC offset is current, so it remains a sound frequency
//! and pulse reference.  Only the date is wrong.
//!
//! The date is therefore reported as the receiver gives it, with the
//! correction alongside rather than folded in.  Silently correcting
//! would hide a real property of the hardware, and the raw value is what
//! a reader comparing against the front panel will see.

use jiff::civil::Date;

/// Days in one GPS week-number epoch: 1024 weeks.
const EPOCH_DAYS: i32 = 1024 * 7;

/// How far a receiver's calendar has slipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rollover {
    /// How many 1024-week epochs the receiver is behind.  Zero means its
    /// date agrees with the reference.
    pub epochs: u32,
}

impl Rollover {
    /// Whole days the receiver is behind.
    pub fn days(self) -> i64 {
        i64::from(self.epochs) * i64::from(EPOCH_DAYS)
    }
}

/// A date as the receiver reported it, with any rollover detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiverDate {
    /// Exactly what the receiver said.
    raw: Date,
    /// The slip, if one was detected.
    rollover: Option<Rollover>,
}

impl ReceiverDate {
    /// Record a date with no rollover check, for when no trustworthy
    /// reference is available.
    pub fn unchecked(raw: Date) -> Self {
        Self {
            raw,
            rollover: None,
        }
    }

    /// Compare against a known-good date, usually the host clock, and
    /// record how many epochs the receiver is behind.
    ///
    /// Only whole multiples count.  A receiver that is merely wrong, or
    /// one whose date is ahead, is left unflagged: this detects the
    /// rollover failure specifically, not clock error in general.
    pub fn checked(raw: Date, reference: Date) -> Self {
        let behind = reference.since(raw).map(|s| s.get_days()).unwrap_or(0);
        let epochs = behind / EPOCH_DAYS;
        let rollover = if behind > 0 && epochs > 0 && behind % EPOCH_DAYS == 0 {
            u32::try_from(epochs).ok().map(|epochs| Rollover { epochs })
        } else {
            None
        };
        Self { raw, rollover }
    }

    /// The date as the receiver reported it.
    pub fn raw(self) -> Date {
        self.raw
    }

    /// The rollover, if one was detected.
    pub fn rollover(self) -> Option<Rollover> {
        self.rollover
    }

    /// The date with any detected rollover added back, or the raw date
    /// when none was detected.
    pub fn corrected(self) -> Date {
        match self.rollover {
            Some(slip) => self
                .raw
                .checked_add(jiff::Span::new().days(slip.days()))
                .unwrap_or(self.raw),
            None => self.raw,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::EPOCH_DAYS;
    use super::ReceiverDate;
    use jiff::civil::date;

    #[test]
    fn the_development_units_slip_is_one_epoch() {
        // Observed: firmware 3704-C reported this date on this day.
        let seen = ReceiverDate::checked(date(2007, 2, 4), date(2026, 9, 20));
        assert_eq!(seen.rollover().expect("a rollover").epochs, 1);
        assert_eq!(seen.corrected(), date(2026, 9, 20));
        assert_eq!(seen.raw(), date(2007, 2, 4));
    }

    #[test]
    fn a_correct_date_is_not_flagged() {
        let seen = ReceiverDate::checked(date(2026, 9, 20), date(2026, 9, 20));
        assert_eq!(seen.rollover(), None);
        assert_eq!(seen.corrected(), date(2026, 9, 20));
    }

    #[test]
    fn two_epochs_are_detected() {
        let raw = date(2026, 9, 20)
            .checked_sub(jiff::Span::new().days(2 * EPOCH_DAYS))
            .expect("in range");
        let seen = ReceiverDate::checked(raw, date(2026, 9, 20));
        assert_eq!(seen.rollover().expect("a rollover").epochs, 2);
        assert_eq!(seen.corrected(), date(2026, 9, 20));
    }

    #[test]
    fn a_date_that_is_merely_wrong_is_not_a_rollover() {
        // Detecting the specific failure beats guessing at clock error.
        for wrong in [date(2020, 1, 1), date(2007, 2, 5), date(2030, 1, 1)] {
            let seen = ReceiverDate::checked(wrong, date(2026, 9, 20));
            assert_eq!(seen.rollover(), None, "{wrong} was flagged");
            assert_eq!(seen.corrected(), wrong);
        }
    }

    #[test]
    fn an_unchecked_date_is_never_flagged() {
        let seen = ReceiverDate::unchecked(date(2007, 2, 4));
        assert_eq!(seen.rollover(), None);
        assert_eq!(seen.corrected(), date(2007, 2, 4));
    }
}
