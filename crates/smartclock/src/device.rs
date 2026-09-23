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
        absent_if_unsupported(self.ask(id))
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

/// A value this receiver cannot give reads as absent, not as a failure.
///
/// Two things mean that, and they mean the same to a reader.  The
/// receiver may refuse with a state error, because the value does not
/// exist yet -- present holdover error while locked.  Or the dialect
/// may have no command for it at all, because this model does not
/// expose it.
///
/// Neither is a reason to abandon a tier.  A Z3805A has no spelling for
/// TFOM in the Z3801 tree, and one `?` on that line used to void every
/// field after it: the snapshot went unstamped and empty, a hundred and
/// twenty-eight rows carrying one frozen timestamp and no readings,
/// while every other value on the tier was there for the asking.
fn absent_if_unsupported<T>(result: Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(Error::Unsupported { .. }) => Ok(None),
        Err(Error::Device { code, .. }) if is_state_refusal(code) => Ok(None),
        Err(e) => Err(e),
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

/// A satellite count as an unsigned number.
///
/// The receiver answers `+7`, parsed as a signed integer because that
/// is what the response type says.  A negative count is not a reading
/// the hardware can produce, so it is floored rather than refused.
fn satellite_count(n: i64) -> u32 {
    u32::try_from(n).unwrap_or(0)
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

/// One step of a tier's pass.
///
/// Naming the steps lets each tier be written as a list, so the number
/// of steps is the length of that list rather than a second constant
/// to keep in agreement with it.  Dispatch is an exhaustive match, so
/// a step added here and nowhere else does not compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Everything on the fast tier, read together.
    Fast,
    /// The satellite counts.
    Satellites,
    /// Oven temperature and current, and the EFC DAC.
    Oscillator,
    /// The subgroup condition registers.
    Registers,
    /// Holdover duration, prediction and present error.
    Holdover,
    /// Position and date.
    Position,
    /// Log count, learned tempco and the powerup register.
    Counters,
}

/// The steps making up each tier's pass, in the order they are run.
///
/// A scheduler takes one step per turn, so a tier costs the link its
/// longest single step rather than its whole pass.  The fast tier is
/// one step on purpose: its fields are compared against each other,
/// and a time interval from one second beside an EFC from the next is
/// a correlation nobody measured.
const fn steps(tier: Tier) -> &'static [Step] {
    match tier {
        Tier::Fast => &[Step::Fast],
        Tier::Medium => &[
            Step::Satellites,
            Step::Oscillator,
            Step::Registers,
            Step::Holdover,
        ],
        Tier::Slow => &[Step::Position, Step::Counters],
    }
}

/// How many steps a pass over this tier takes.
pub fn step_count(tier: Tier) -> usize {
    steps(tier).len()
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
    ///
    /// This runs the tier's whole pass in one go.  A scheduler sharing
    /// the link with a faster tier wants [`Device::poll_step`] instead.
    pub fn poll(&mut self, tier: Tier, into: &mut Snapshot, now: Timestamp) -> Result<()> {
        for step in 0..step_count(tier) {
            self.poll_step(tier, step, into, now)?;
        }
        Ok(())
    }

    /// Refresh one step of `tier`'s pass.
    ///
    /// Steps are numbered from zero and are meant to be run in order.
    /// Success is recorded only on the last one: a pass is as old as
    /// its slowest field, so stamping it partway through would call
    /// fields fresh that had not been read yet.
    ///
    /// A step index past the end of the tier reads nothing and is not
    /// an error, so a caller that has lost its place cannot wedge.
    pub fn poll_step(
        &mut self,
        tier: Tier,
        step: usize,
        into: &mut Snapshot,
        now: Timestamp,
    ) -> Result<()> {
        let steps = steps(tier);
        let Some(step_kind) = steps.get(step) else {
            return Ok(());
        };
        match step_kind {
            Step::Fast => self.poll_fast(into)?,
            Step::Satellites => self.poll_satellites(into)?,
            Step::Oscillator => self.poll_oscillator(into)?,
            Step::Registers => self.poll_registers(into)?,
            Step::Holdover => self.poll_holdover(into)?,
            Step::Position => self.poll_position(into, now)?,
            Step::Counters => self.poll_counters(into)?,
        }
        into.at = now;
        if step + 1 == steps.len() {
            into.polled.succeeded(tier, now);
        }
        into.settle_freshness();
        Ok(())
    }

    fn poll_fast(&mut self, into: &mut Snapshot) -> Result<()> {
        into.mode = absent_if_unsupported(self.mode())?;
        into.tfom = absent_if_unsupported(self.tfom())?;
        into.ffom = absent_if_unsupported(self.ffom())?;
        into.time_interval = absent_if_unsupported(self.time_interval())?.flatten();
        into.efc = absent_if_unsupported(self.efc())?;
        into.hardware = absent_if_unsupported(self.hardware_condition())?;
        into.holdover_waiting = absent_if_unsupported(self.holdover_waiting())?;
        into.time = absent_if_unsupported(self.time())?;
        Ok(())
    }

    /// The satellite counts, which the status screen also prints.
    ///
    /// Two short queries, about 80 ms, against the 1.5 s the screen
    /// costs.  Reading them here is what lets the screen sit on the
    /// slow tier without the counts going stale with it.
    fn poll_satellites(&mut self, into: &mut Snapshot) -> Result<()> {
        into.tracking = absent_if_unsupported(self.tracking_count())?.map(satellite_count);
        into.visible = absent_if_unsupported(self.visible_count())?.map(satellite_count);
        Ok(())
    }

    fn poll_oscillator(&mut self, into: &mut Snapshot) -> Result<()> {
        // Temperature and oven current sit here rather than on the fast
        // tier: they move slowly, and the fast tier is already close to
        // its budget.
        into.temperature = absent_if_unsupported(self.temperature())?.flatten();
        into.oven_current = absent_if_unsupported(self.oven_current())?.flatten();
        into.efc_dac = absent_if_unsupported(self.efc_dac())?.flatten();
        Ok(())
    }

    /// The subgroup condition registers.
    ///
    /// A condition register is read in real time and holds nothing, so
    /// sampling one every ten seconds misses transitions -- the event
    /// registers catch those, and reading an event register clears it,
    /// which a logger has no business doing.  What survives a poll here
    /// is the receiver's steady state, which is what the history is
    /// for.
    fn poll_registers(&mut self, into: &mut Snapshot) -> Result<()> {
        into.alarm = absent_if_unsupported(self.alarm_condition())?;
        into.operation = absent_if_unsupported(self.operation_condition())?;
        into.holdover_state = absent_if_unsupported(self.holdover_condition())?;
        Ok(())
    }

    fn poll_holdover(&mut self, into: &mut Snapshot) -> Result<()> {
        let holdover = self.holdover_duration()?;
        into.holdover_duration = Some(holdover);
        into.holdover_predicted = absent_if_unsupported(self.holdover_predicted())?.flatten();
        // Only asked for while it exists.  Present holdover error is
        // refused with -230 whenever the receiver is locked, and a
        // refusal is not free: the session reads the error queue to
        // explain it, so a question we already know the answer to cost
        // two round trips of a tier that is against its wire budget,
        // on every medium poll, for the life of the daemon.  It also set
        // the command-error bit in the standard event status register
        // each time, which made that register a record of our own
        // manners rather than of the receiver's.
        into.holdover_present = if holdover.active {
            self.holdover_present()?
        } else {
            None
        };
        Ok(())
    }

    /// Every one of these is wrapped, including the two that already
    /// return an Option.  A receiver that has never had a fix refuses
    /// its date with -230, and a bare `?` on that line aborts the rest
    /// of the tier: the counters below would report themselves missing
    /// when they were merely unreached.
    fn poll_position(&mut self, into: &mut Snapshot, now: Timestamp) -> Result<()> {
        into.position = absent_if_unsupported(self.position())?.flatten();
        let today = now.to_zoned(jiff::tz::TimeZone::UTC).date();
        into.date = absent_if_unsupported(self.date(today))?;
        Ok(())
    }

    fn poll_counters(&mut self, into: &mut Snapshot) -> Result<()> {
        into.log_count = absent_if_unsupported(self.log_count())?;
        into.oven_tempco = absent_if_unsupported(self.oven_tempco())?.flatten();
        // All three of its bits are set during startup and then stay,
        // so asking on every medium poll spent a round trip to be told
        // what it said last time.  The medium tier is the one against
        // its budget; this belongs with the other things that barely
        // move.
        into.powerup = absent_if_unsupported(self.powerup_condition())?;
        Ok(())
    }
    /// The status screen, read on request rather than on a schedule.
    ///
    /// The slowest read the receiver has: 1574 bytes, 0.94 s of wire
    /// time at 19200 and 1.5 s measured, the rest being the receiver
    /// composing it -- four fast passes' worth, for one screen.
    ///
    /// Nothing polls it, because after the satellite counts moved to
    /// `:GPS:SATellite:TRACking:COUNt?` and
    /// `:GPS:SATellite:VISible:PREDicted:COUNt?` the only fields left
    /// that appear nowhere else are per-satellite elevation, azimuth
    /// and signal strength.  The health monitor line is the hardware
    /// condition register rendered coarsely, and that register is on
    /// the fast tier; the bracketed synchronisation and acquisition
    /// text is `:SYNChronization:STATe?` and the operation register.
    /// So a sky plot is read when someone is looking at one.
    pub fn poll_screen(&mut self, into: &mut Snapshot, now: Timestamp) -> Result<()> {
        into.screen = absent_if_unsupported(self.screen())?;
        into.at = now;
        into.settle_freshness();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Step;
    use super::absent_if_unsupported;
    use super::steps;
    use crate::command::CommandId;
    use crate::snapshot::Tier;

    use super::datum_for;
    use super::dialect_for;
    use crate::command::Dialect;
    use crate::types::Datum;

    #[test]
    fn every_step_belongs_to_exactly_one_tier() {
        // A missing match arm is a compile error; a variant that no
        // tier schedules is not, and would read nothing forever.
        let scheduled: Vec<Step> = Tier::ALL.iter().flat_map(|t| steps(*t)).copied().collect();
        // The screen is read on request, not on a schedule.
        for step in [
            Step::Fast,
            Step::Satellites,
            Step::Oscillator,
            Step::Registers,
            Step::Holdover,
            Step::Position,
            Step::Counters,
        ] {
            assert_eq!(
                scheduled.iter().filter(|s| **s == step).count(),
                1,
                "{step:?}"
            );
        }
        assert_eq!(scheduled.len(), 7);
    }

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
    #[test]
    fn a_command_this_model_lacks_is_absent_rather_than_a_failure() {
        // A Z3805A has no TFOM in the Z3801 tree.  Before this, the `?`
        // on that one line abandoned the rest of the fast tier: the
        // snapshot was never stamped and never filled, so a hundred and
        // twenty-eight rows arrived carrying one frozen timestamp and
        // no readings, while FFOM, EFC and the rest were there for the
        // asking.
        let missing: crate::error::Result<u8> = Err(crate::error::Error::Unsupported {
            dialect: "z3801",
            operation: CommandId::Tfom,
        });
        assert_eq!(
            absent_if_unsupported(missing).expect("must not propagate"),
            None
        );

        // A refusal means the same to a reader and is treated the same.
        let refused: crate::error::Result<u8> = Err(crate::error::Error::Device {
            code: -230,
            message: "Data corrupt or stale".to_owned(),
        });
        assert_eq!(absent_if_unsupported(refused).expect("nor this"), None);

        // Anything else still is a failure: a receiver that stopped
        // answering must not read as a receiver without the feature.
        let broken: crate::error::Result<u8> = Err(crate::error::Error::Parse {
            reply: "nonsense".to_owned(),
            expected: "a number",
        });
        assert!(absent_if_unsupported(broken).is_err());

        assert_eq!(absent_if_unsupported(Ok(3u8)).expect("a value"), Some(3));
    }
}
