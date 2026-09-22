//! Typed access to one receiver.
//!
//! Sits on a [`Session`] and resolves logical operations through the
//! dialect table, so a caller names what it wants rather than how a
//! particular firmware spells it.

use crate::command::CommandId;
use crate::command::Dialect;
use crate::control::Control;
use crate::error::Error;
use crate::error::Result;
use crate::error::is_state_refusal;
use crate::parse;
use crate::parse::Identity;
use crate::rollover::ReceiverDate;
use crate::screen;
use crate::screen::Screen;
use crate::session::Session;
use crate::snapshot::Snapshot;
use crate::snapshot::Tier;
use crate::transport::Transport;
use crate::types::AlarmCondition;
use crate::types::Datum;
use crate::types::EfcPercent;
use crate::types::Ffom;
use crate::types::HardwareCondition;
use crate::types::HoldoverCondition;
use crate::types::HoldoverDuration;
use crate::types::HoldoverWaitReason;
use crate::types::OperationCondition;
use crate::types::Position;
use crate::types::PowerupCondition;
use crate::types::Seconds;
use crate::types::SmartClockMode;
use crate::types::Tfom;
use crate::types::TimeOfDay;

use jiff::Timestamp;
use jiff::civil::Date;

/// One receiver, addressed by logical operation.
#[derive(Debug)]
pub struct Device<T: Transport> {
    session: Session<T>,
    dialect: Dialect,
    identity: Identity,
}

impl<T: Transport> Device<T> {
    /// Identify the receiver and choose a dialect for it.
    ///
    /// The session is synchronised first, since the receiver may be
    /// mid-reply from whatever spoke to it last.
    pub fn open(mut session: Session<T>) -> Result<Self> {
        session.sync()?;
        let reply = session.query("*IDN?")?;
        let identity = parse::identity(reply.one_line("an identity")?)?;
        let dialect = dialect_for(&identity.model);
        Ok(Self {
            session,
            dialect,
            identity,
        })
    }

    /// Override the dialect chosen from `*IDN?`.
    pub fn with_dialect(mut self, dialect: Dialect) -> Self {
        self.dialect = dialect;
        self
    }

    /// What the receiver said it is.
    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    /// Which command tree is in use.
    pub fn dialect(&self) -> Dialect {
        self.dialect
    }

    /// The underlying session, for raw commands.
    pub fn session(&mut self) -> &mut Session<T> {
        &mut self.session
    }

    /// A handle for changing receiver state.
    ///
    /// Separate from the read paths on purpose: reaching a control
    /// command means naming this, so nothing that only meant to read
    /// can send one by accident.
    pub fn control(&mut self) -> Control<'_, T> {
        Control::new(&mut self.session, self.dialect)
    }

    /// Send one logical operation and return its single reply line.
    fn ask(&mut self, id: CommandId) -> Result<String> {
        // Naming the operation, not "the requested command": a typed
        // error carrying a constant string is worse than no error.
        let spec = self.dialect.spec(id).ok_or(Error::Unsupported {
            dialect: self.dialect.name(),
            operation: id,
        })?;
        let scpi = spec.scpi;
        let reply = self.session.query(scpi)?;
        Ok(reply.one_line("a single line")?.to_owned())
    }

    /// As [`Device::ask`], but a receiver declining on state yields
    /// `None` rather than an error.
    ///
    /// -221 and -230 mean the value does not exist right now, such as
    /// present holdover error while locked.  That is an answer, not a
    /// failure, and a monitor should show it as absent.
    fn ask_optional(&mut self, id: CommandId) -> Result<Option<String>> {
        match self.ask(id) {
            Ok(line) => Ok(Some(line)),
            Err(Error::Device { code, .. }) if is_state_refusal(code) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Which mode the disciplining loop is in.
    pub fn mode(&mut self) -> Result<SmartClockMode> {
        let line = self.ask(CommandId::SyncState)?;
        SmartClockMode::parse(&line).ok_or(Error::Parse {
            reply: line.clone(),
            expected: "a SmartClock mode",
        })
    }

    /// Time figure of merit.
    pub fn tfom(&mut self) -> Result<Tfom> {
        let line = self.ask(CommandId::Tfom)?;
        to_u8(&line).and_then(Tfom::new).ok_or(Error::Parse {
            reply: line,
            expected: "a TFOM in 1..=9",
        })
    }

    /// Frequency figure of merit.
    pub fn ffom(&mut self) -> Result<Ffom> {
        let line = self.ask(CommandId::Ffom)?;
        to_u8(&line).and_then(Ffom::new).ok_or(Error::Parse {
            reply: line,
            expected: "an FFOM in 0..=9",
        })
    }

    /// Interval between the receiver's 1 PPS and the GPS 1 PPS.
    pub fn time_interval(&mut self) -> Result<Option<Seconds>> {
        self.ask_optional(CommandId::Tinterval)?
            .map(|l| parse::seconds(&l))
            .transpose()
    }

    /// Oscillator control voltage, as a share of its range.
    pub fn efc(&mut self) -> Result<EfcPercent> {
        let line = self.ask(CommandId::Efc)?;
        parse::efc(&line)
    }

    /// Internal temperature, in degrees Celsius.
    ///
    /// Undocumented.  Worth having because an OCXO's control voltage
    /// moves with temperature, so EFC drift cannot be read as ageing
    /// without it.
    pub fn temperature(&mut self) -> Result<Option<f64>> {
        self.ask_optional(CommandId::Temperature)?
            .map(|l| parse::real(&l))
            .transpose()
    }

    /// Oven current.  Undocumented.
    pub fn oven_current(&mut self) -> Result<Option<f64>> {
        self.ask_optional(CommandId::OvenCurrent)?
            .map(|l| parse::real(&l))
            .transpose()
    }

    /// The temperature coefficient the receiver has learned for its
    /// oscillator.  Undocumented.
    ///
    /// Unlike the temperature and the oven current, this is not a
    /// measurement but a model: the receiver's own estimate of how the
    /// crystal responds.  An estimate that moves over months is the
    /// receiver saying the crystal has changed, which no instantaneous
    /// reading shows.
    pub fn oven_tempco(&mut self) -> Result<Option<f64>> {
        self.ask_optional(CommandId::OvenTempco)?
            .map(|l| parse::real(&l))
            .transpose()
    }

    /// EFC as the raw DAC code.  Undocumented.
    pub fn efc_dac(&mut self) -> Result<Option<u32>> {
        let Some(line) = self.ask_optional(CommandId::EfcAbsolute)? else {
            return Ok(None);
        };
        let code = parse::int(&line)?;
        Ok(u32::try_from(code).ok())
    }

    /// The hardware condition register.
    pub fn hardware_condition(&mut self) -> Result<HardwareCondition> {
        self.register(CommandId::HardwareCondition)
            .map(HardwareCondition::from_bits)
    }

    /// The alarm condition register, which `*STB?` reads.
    ///
    /// Non-destructive, unlike every event register, which is the whole
    /// reason it is what gets polled: it reports which groups have
    /// latched something without taking the latch away.
    pub fn alarm_condition(&mut self) -> Result<AlarmCondition> {
        self.register(CommandId::Stb).map(AlarmCondition::from_bits)
    }

    /// The operation condition register.
    pub fn operation_condition(&mut self) -> Result<OperationCondition> {
        self.register(CommandId::OperCondition)
            .map(OperationCondition::from_bits)
    }

    /// The holdover condition register.
    pub fn holdover_condition(&mut self) -> Result<HoldoverCondition> {
        self.register(CommandId::HoldoverCondition)
            .map(HoldoverCondition::from_bits)
    }

    /// The powerup condition register.
    pub fn powerup_condition(&mut self) -> Result<PowerupCondition> {
        self.register(CommandId::PowerupCondition)
            .map(PowerupCondition::from_bits)
    }

    /// Read a status register as a bare 16-bit word.
    ///
    /// Condition registers only.  Reading an event register clears it,
    /// which would take the latched bit away from whatever else is
    /// watching and, through the summary bits, retract the receiver's
    /// own alarm; a logger must not do that as a side effect of
    /// logging.
    fn register(&mut self, id: CommandId) -> Result<u16> {
        let line = self.ask(id)?;
        let bits = parse::int(&line)?;
        u16::try_from(bits).map_err(|_| Error::Parse {
            reply: line,
            expected: "a 16-bit register",
        })
    }

    /// Why the receiver has not left holdover.
    pub fn holdover_waiting(&mut self) -> Result<HoldoverWaitReason> {
        let line = self.ask(CommandId::HoldoverWaiting)?;
        HoldoverWaitReason::parse(&line).ok_or(Error::Parse {
            reply: line.clone(),
            expected: "a holdover wait reason",
        })
    }

    /// How long holdover has lasted, and whether it is current.
    pub fn holdover_duration(&mut self) -> Result<HoldoverDuration> {
        let line = self.ask(CommandId::HoldoverDuration)?;
        parse::holdover_duration(&line)
    }

    /// Predicted time error over a day of holdover.
    pub fn holdover_predicted(&mut self) -> Result<Option<Seconds>> {
        Ok(self
            .ask_optional(CommandId::HoldoverUncPred)?
            .map(|l| parse::holdover_duration(&l))
            .transpose()?
            .map(|d| d.elapsed))
    }

    /// Time error accumulated so far in holdover.  Absent unless the
    /// receiver is in holdover.
    pub fn holdover_present(&mut self) -> Result<Option<Seconds>> {
        self.ask_optional(CommandId::HoldoverUncNow)?
            .map(|l| parse::seconds(&l))
            .transpose()
    }

    /// How many satellites are being used.
    pub fn tracking_count(&mut self) -> Result<i64> {
        let line = self.ask(CommandId::SatTrackingCount)?;
        parse::int(&line)
    }

    /// How many the almanac expects to be visible.
    pub fn visible_count(&mut self) -> Result<i64> {
        let line = self.ask(CommandId::SatVisibleCount)?;
        parse::int(&line)
    }

    /// The averaged antenna position.
    pub fn position(&mut self) -> Result<Option<Position>> {
        let datum = datum_for(&self.identity.model);
        self.ask_optional(CommandId::PositionAvg)?
            .map(|l| parse::position(&l, datum))
            .transpose()
    }

    /// The receiver's date, checked against `today` for a GPS week
    /// rollover.
    pub fn date(&mut self, today: Date) -> Result<ReceiverDate> {
        let line = self.ask(CommandId::Date)?;
        Ok(ReceiverDate::checked(parse::ymd(&line)?, today))
    }

    /// The receiver's time of day.
    pub fn time(&mut self) -> Result<TimeOfDay> {
        let line = self.ask(CommandId::Time)?;
        parse::hms(&line)
    }

    /// How many diagnostic log entries are held.
    pub fn log_count(&mut self) -> Result<i64> {
        let line = self.ask(CommandId::LogCount)?;
        parse::int(&line)
    }

    /// The whole status screen, scraped.
    ///
    /// This is the only source of per-satellite elevation, azimuth and
    /// signal strength, and it is the most expensive query the receiver
    /// offers: roughly 1.8 KB, about a second of wire time at 19200.
    pub fn screen(&mut self) -> Result<Screen> {
        let spec = self
            .dialect
            .spec(CommandId::StatusScreen)
            .ok_or(Error::Unsupported {
                dialect: self.dialect.name(),
                operation: CommandId::StatusScreen,
            })?;
        let reply = self.session.query(spec.scpi)?;
        Ok(screen::parse(&reply.lines.join("\n")))
    }
}

fn to_u8(line: &str) -> Option<u8> {
    parse::int(line).ok().and_then(|n| u8::try_from(n).ok())
}

/// Pick a command tree from the model in `*IDN?`.
fn dialect_for(model: &str) -> Dialect {
    let model = model.to_ascii_uppercase();
    if model.starts_with("Z38") {
        Dialect::Z3801
    } else {
        Dialect::Hp58503
    }
}

/// Which vertical datum a model's heights use.
///
/// The 58503A and 59551A report height above mean sea level; the 58503B
/// reports it above the GPS ellipsoid.  Getting this wrong misplaces a
/// position by tens of metres vertically.
fn datum_for(model: &str) -> Datum {
    match model.to_ascii_uppercase().as_str() {
        "58503B" => Datum::Ellipsoid,
        _ => Datum::MeanSeaLevel,
    }
}

/// Polling one tier's worth of fields into a snapshot.
impl<T: Transport> Device<T> {
    /// Refresh the fields belonging to `tier`.
    ///
    /// A field the receiver declines on state is set to `None` rather
    /// than left at its previous value, because a stale reading is
    /// worse than an absent one.  A transport failure aborts the tier
    /// and propagates.
    ///
    /// Only this tier's success is recorded.  The others keep the time
    /// of their own last success, so a caller can tell how old each
    /// group of fields is rather than reading one timestamp that the
    /// fastest tier keeps refreshing on everyone's behalf.
    pub fn poll(&mut self, tier: Tier, into: &mut Snapshot, now: Timestamp) -> Result<()> {
        match tier {
            Tier::Fast => self.poll_fast(into)?,
            Tier::Medium => self.poll_medium(into)?,
            Tier::Slow => self.poll_slow(into, now)?,
        }
        into.at = now;
        into.polled.succeeded(tier, now);
        into.settle_freshness();
        Ok(())
    }

    fn poll_fast(&mut self, into: &mut Snapshot) -> Result<()> {
        into.mode = Some(self.mode()?);
        into.tfom = Some(self.tfom()?);
        into.ffom = Some(self.ffom()?);
        into.time_interval = self.time_interval()?;
        into.efc = Some(self.efc()?);
        into.hardware = Some(self.hardware_condition()?);
        into.holdover_waiting = Some(self.holdover_waiting()?);
        into.time = Some(self.time()?);
        Ok(())
    }

    fn poll_medium(&mut self, into: &mut Snapshot) -> Result<()> {
        // Temperature and oven current sit here rather than on the fast
        // tier: they move slowly, and the fast tier is already close to
        // its budget.
        into.temperature = self.temperature()?;
        into.oven_current = self.oven_current()?;
        into.efc_dac = self.efc_dac()?;
        // The subgroup condition registers.  A condition register is
        // read in real time and holds nothing, so sampling one every
        // ten seconds misses transitions -- the event registers catch
        // those, and reading an event register clears it, which a
        // logger has no business doing.  What survives a poll here is
        // the receiver's steady state, which is what the history is
        // for.
        into.alarm = Some(self.alarm_condition()?);
        into.operation = Some(self.operation_condition()?);
        into.holdover_state = Some(self.holdover_condition()?);
        into.powerup = Some(self.powerup_condition()?);
        into.holdover_duration = Some(self.holdover_duration()?);
        into.holdover_predicted = self.holdover_predicted()?;
        into.holdover_present = self.holdover_present()?;
        into.screen = Some(self.screen()?);
        Ok(())
    }

    fn poll_slow(&mut self, into: &mut Snapshot, now: Timestamp) -> Result<()> {
        into.position = self.position()?;
        let today = now.to_zoned(jiff::tz::TimeZone::UTC).date();
        let date = self.date(today)?;
        into.date = Some(date);
        into.log_count = Some(self.log_count()?);
        into.oven_tempco = self.oven_tempco()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::datum_for;
    use super::dialect_for;
    use crate::command::Dialect;
    use crate::types::Datum;

    #[test]
    fn the_model_chooses_the_command_tree() {
        assert_eq!(dialect_for("58503A"), Dialect::Hp58503);
        assert_eq!(dialect_for("58503B"), Dialect::Hp58503);
        assert_eq!(dialect_for("59551A"), Dialect::Hp58503);
        assert_eq!(dialect_for("Z3801A"), Dialect::Z3801);
        assert_eq!(dialect_for("z3816a"), Dialect::Z3801);
        // An unknown model gets the tree we can actually test against.
        assert_eq!(dialect_for("58540A"), Dialect::Hp58503);
    }

    #[test]
    fn the_model_chooses_the_vertical_datum() {
        assert_eq!(datum_for("58503A"), Datum::MeanSeaLevel);
        assert_eq!(datum_for("59551A"), Datum::MeanSeaLevel);
        assert_eq!(datum_for("58503B"), Datum::Ellipsoid);
    }
}
