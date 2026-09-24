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
/// `:SYSTem:PRESet`, anything under `:SYSTem:COMMunicate`,
/// `:DIAGnostic:ERASe`, and setting `:SYSTem:LANGuage`, which selects
/// "INSTALL" or "PRIMARY" (097-59551-02 4-15).  Serial settings persist
/// across power cycles, so a changed one strands the link.  There is no
/// override: the daemon's `--allow-dangerous` is the one route to
/// these, and a deliberate one.
///
/// Matched on the mandatory abbreviations -- SYST, PRES, COMM, ERAS,
/// LANG -- anywhere in the string, so every legal spelling and a
/// compound command hiding one after a semicolon are both caught.
pub fn forbidden(scpi: &str) -> Option<&'static str> {
    let upper = scpi.to_ascii_uppercase();
    let bare_query = upper.ends_with('?') && !upper.contains(char::is_whitespace);
    if upper.contains("COMM") {
        Some(":SYSTem:COMMunicate")
    } else if upper.contains("SYST") && upper.contains("PRES") {
        Some(":SYSTem:PRESet")
    } else if upper.contains("ERAS") {
        Some(":DIAGnostic:ERASe")
    } else if upper.contains("LANG") && !bare_query {
        Some(":SYSTem:LANGuage")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::forbidden;

    #[test]
    fn the_four_are_refused_however_they_are_spelled() {
        for scpi in [
            ":SYSTem:PRESet",
            ":syst:pres",
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
        ] {
            assert_eq!(forbidden(scpi), None, "{scpi}");
        }
    }
}
