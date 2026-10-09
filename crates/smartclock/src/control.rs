//! Commands that change the receiver.
//!
//! Reached only through [`Device::control`], so a read-only path cannot
//! issue one by accident: the borrow is explicit and the type name says
//! what it is.  Whether a caller may build one at all is decided above
//! this layer -- in the daemon, by a flag given at startup.

use crate::command::CommandId;
use crate::command::Dialect;
use crate::error::Error;
use crate::error::Result;
use crate::session::Session;
use crate::transport::Transport;
use crate::types::Seconds;

/// A handle for changing receiver state.
#[derive(Debug)]
pub struct Control<'a, T: Transport> {
    session: &'a mut Session<T>,
    dialect: Dialect,
}

impl<'a, T: Transport> Control<'a, T> {
    /// Build a handle.  Callers normally use [`Device::control`].
    ///
    /// [`Device::control`]: crate::device::Device::control
    pub fn new(session: &'a mut Session<T>, dialect: Dialect) -> Self {
        Self { session, dialect }
    }

    /// Send a command that takes no argument.
    fn send(&mut self, id: CommandId) -> Result<()> {
        let scpi = self.spec(id)?;
        self.session.query(scpi)?;
        Ok(())
    }

    /// Send a command with an argument appended.
    fn send_with(&mut self, id: CommandId, argument: &str) -> Result<()> {
        let scpi = self.spec(id)?;
        self.session.query(&format!("{scpi} {argument}"))?;
        Ok(())
    }

    fn spec(&self, id: CommandId) -> Result<&'static str> {
        self.dialect
            .spec(id)
            .map(|s| s.scpi)
            .ok_or(Error::Unsupported {
                dialect: self.dialect.name(),
                operation: id,
            })
    }

    /// Force the receiver into holdover and keep it there.
    ///
    /// It stays until [`Control::recover_from_holdover`], not until
    /// conditions improve.  Coming back out is what makes the receiver
    /// sweep its oscillator control across a wide range, which is the
    /// only way to measure how much frequency one step of it buys.
    pub fn initiate_holdover(&mut self) -> Result<()> {
        self.send(CommandId::HoldoverInitiate)
    }

    /// Leave a manually initiated holdover.
    pub fn recover_from_holdover(&mut self) -> Result<()> {
        self.send(CommandId::HoldoverRecover)
    }

    /// Recover even though the time interval limit is exceeded.
    pub fn ignore_recovery_limit(&mut self) -> Result<()> {
        self.send(CommandId::HoldoverLimitIgnore)
    }

    /// Realign the output 1 PPS to the GPS 1 PPS at once.
    pub fn align_pulse(&mut self) -> Result<()> {
        self.send(CommandId::SyncImmediate)
    }

    /// Begin a position survey.
    pub fn survey(&mut self) -> Result<()> {
        self.send(CommandId::SurveyOnce)
    }

    /// Set the elevation mask, in degrees.
    pub fn set_elevation_mask(&mut self, degrees: u8) -> Result<()> {
        if degrees > 90 {
            return Err(Error::Parse {
                reply: degrees.to_string(),
                expected: "an elevation mask of 90 degrees or less",
            });
        }
        self.send_with(CommandId::ElevationMaskSet, &degrees.to_string())
    }

    /// Set the antenna cable delay.
    ///
    /// The manual warns that changing this while locked can throw the
    /// receiver into holdover.
    pub fn set_antenna_delay(&mut self, delay: Seconds) -> Result<()> {
        self.send_with(
            CommandId::AntennaDelaySet,
            &format!("{:E}", delay.as_secs()),
        )
    }

    /// Set the holdover duration threshold, in seconds.
    pub fn set_holdover_threshold(&mut self, seconds: u32) -> Result<()> {
        self.send_with(CommandId::HoldoverThreshSet, &seconds.to_string())
    }

    /// Restore every setting to its factory value.
    ///
    /// Dangerous: this discards the surveyed position, the antenna
    /// delay and the elevation mask, and the receiver then has to
    /// re-acquire and re-survey.
    pub fn system_preset(&mut self) -> Result<()> {
        self.send(CommandId::SystemPreset)
    }
}

/// Which of the commands a tool talking to the receiver directly must
/// never send `scpi` is, or `None` if it is none of them.
///
/// `:SYSTem:PRESet`, the undocumented `:SYSTem:PON` (a restart that
/// discards the state a warm restart would keep; `docs/firmware/restart.md`,
/// "Restarting"), anything under `:SYSTem:COMMunicate`,
/// `:DIAGnostic:ERASe`, and setting `:SYSTem:LANGuage`, which selects
/// "INSTALL" or "PRIMARY" (097-59551-02 4-15).  Serial settings persist
/// across power cycles, so a changed one strands the link.  There is no
/// override: the daemon's `--allow-dangerous` is the one route to
/// these, and a deliberate one.
///
/// Matched keyword by keyword with `names`, so every legal spelling and
/// a compound command hiding one after a semicolon are both caught,
/// and `:KENneth:PRESent?` is not taken for `PRESet`.
pub fn forbidden(scpi: &str) -> Option<&'static str> {
    let bare_query = scpi.ends_with('?') && !scpi.contains(char::is_whitespace);
    if names(scpi, "COMMunicate") {
        Some(":SYSTem:COMMunicate")
    } else if names(scpi, "SYSTem") && names(scpi, "PRESet") {
        Some(":SYSTem:PRESet")
    } else if names(scpi, "SYSTem") && names(scpi, "PON") {
        Some(":SYSTem:PON")
    } else if names(scpi, "ERASe") {
        Some(":DIAGnostic:ERASe")
    } else if names(scpi, "LANGuage") && !bare_query {
        Some(":SYSTem:LANGuage")
    } else {
        None
    }
}

/// Whether one of the keywords of `scpi` spells `keyword`, written as
/// the manuals write it, short form upper case (`PRESet`).
///
/// A keyword of `scpi` is a run of letters and digits; a numeric suffix
/// is set aside, and any length from the short form to the long one,
/// in any case, spells it.  That is more than SCPI accepts, which
/// errs toward finding the keyword: `PRES`, `PRESE` and `preset1` spell
/// `PRESet`, `PRESent` and `PRE` do not.
pub fn names(scpi: &str, keyword: &str) -> bool {
    let long = keyword.to_ascii_uppercase();
    let short: String = keyword
        .chars()
        .filter(|letter| !letter.is_ascii_lowercase())
        .collect();
    scpi.split(|letter: char| !letter.is_ascii_alphanumeric())
        .map(|word| word.trim_end_matches(|letter: char| letter.is_ascii_digit()))
        .filter(|word| !word.is_empty())
        .any(|word| {
            let word = word.to_ascii_uppercase();
            word.starts_with(&short) && long.starts_with(&word)
        })
}

#[cfg(test)]
mod tests {
    use super::forbidden;
    use super::names;

    #[test]
    fn a_keyword_is_named_by_any_spelling_from_short_to_long() {
        for scpi in [
            ":SYST:PRES",
            ":syst:preset",
            "*IDN?;:SYSTem:PRESE",
            ":X:PRESet1",
        ] {
            assert!(names(scpi, "PRESet"), "{scpi}");
        }
        for scpi in [":KENneth:PRESent?", ":PRE", ":REPRESet", ":PRESETS"] {
            assert!(!names(scpi, "PRESet"), "{scpi}");
        }
    }

    #[test]
    fn the_five_are_refused_however_they_are_spelled() {
        for scpi in [
            ":SYSTem:PRESet",
            ":syst:pres",
            ":SYSTem:PON",
            ":syst:pon",
            "*IDN?;:SYST:PON",
            ":SYSTem:COMMunicate:SERial:BAUD 9600",
            ":SYST:COMM:SER:BAUD?",
            ":DIAGnostic:ERASe",
            ":SYSTem:LANGuage \"INSTALL\"",
            "*IDN?;:SYST:PRES",
        ] {
            assert!(forbidden(scpi).is_some(), "{scpi}");
        }
    }

    #[test]
    fn ordinary_commands_are_not() {
        for scpi in [
            "*IDN?",
            ":SYSTem:LANGuage?",
            ":STATus:PRESet",
            ":SYNChronization:TINTerval?",
            ":DIAGnostic:LOG:READ? 1",
            ":SYSTem:STATus?;:KENneth:PRESent?",
        ] {
            assert_eq!(forbidden(scpi), None, "{scpi}");
        }
    }
}
