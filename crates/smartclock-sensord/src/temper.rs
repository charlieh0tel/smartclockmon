//! `TEMPerGold` and `TEMPerHUM` USB sticks from `PCsensor`, spoken to directly
//! through `temper-hid`: on Linux over hidraw, on Windows over hidapi.
//!
//! A stick is held open between reads, which holds its lock -- another
//! process opening it is refused -- and spares the second it takes to
//! settle after each open.  After a failed read it is closed and looked
//! for again on the next, so one unplugged and plugged back in is
//! picked up.

use std::error::Error;
use std::path::PathBuf;

use temper_hid::hid;
use temper_hid::hid::Device;
use temper_hid::protocol::Stick;

use crate::sensor::ConfigError;
use crate::sensor::Measure;
use crate::sensor::Name;
use crate::sensor::Quantity;
use crate::sensor::ReadError;
use crate::sensor::Source;

/// What a stick configured without a path is described as.
const FIRST_FOUND: &str = "the first TEMPer stick found";

/// One configured stick.
#[derive(Debug)]
pub struct Temper {
    name: Name,
    /// The stick's node, or `None` for the first found that no other
    /// process holds.
    path: Option<PathBuf>,
    /// How it was configured, for the log.
    origin: String,
    /// The stick, while it is open.
    open: Option<Open>,
    /// Whether finding the first of several has been said already.
    told_several: bool,
}

/// A stick being read.
#[derive(Debug)]
struct Open {
    stick: Stick<Device>,
    /// Its firmware's name for itself, which says the model.
    firmware: String,
}

/// Parse a stick configured as `NAME` or `NAME=PATH`.
pub fn parse(spec: &str) -> Result<Temper, ConfigError> {
    let (name, path) = match spec.split_once('=') {
        Some((_, "")) => return Err(ConfigError::Form(spec.to_owned())),
        Some((name, path)) => (name, Some(PathBuf::from(path.trim()))),
        None => (spec, None),
    };
    let origin = path
        .as_ref()
        .map_or_else(|| FIRST_FOUND.to_owned(), |path| path.display().to_string());
    Ok(Temper {
        name: Name::new(name.trim())?,
        path,
        origin,
        open: None,
        told_several: false,
    })
}

impl Temper {
    /// The stick, opened now if it is not open already.
    fn opened(&mut self) -> Result<&mut Open, ReadError> {
        let open = match self.open.take() {
            Some(open) => open,
            None => self.open_now()?,
        };
        Ok(self.open.insert(open))
    }

    /// Open the stick, as configured, and ask what it is.
    fn open_now(&mut self) -> Result<Open, ReadError> {
        let (path, mut stick) = match &self.path {
            Some(path) => (path.clone(), Stick::open(path).map_err(stick_error)?),
            None => self.first_free()?,
        };
        let firmware = stick.firmware().map_err(stick_error)?.to_string();
        eprintln!(
            "smartclock-sensord: {}: opened {} ({firmware})",
            self.name,
            path.display()
        );
        Ok(Open { stick, firmware })
    }

    /// The first stick no other process holds, saying once if there
    /// were several to choose from.
    fn first_free(&mut self) -> Result<(PathBuf, Stick<Device>), ReadError> {
        let found = hid::discover().map_err(stick_error)?;
        if found.len() > 1 && !self.told_several {
            self.told_several = true;
            let paths: Vec<String> = found.iter().map(|p| p.display().to_string()).collect();
            eprintln!(
                "smartclock-sensord: {}: several TEMPer sticks found, taking the first free: {}",
                self.name,
                paths.join(", ")
            );
        }
        let mut refused = None;
        for path in found {
            match Stick::open(&path) {
                Ok(stick) => return Ok((path, stick)),
                Err(e) => refused = Some(e),
            }
        }
        Err(stick_error(refused.unwrap_or(hid::Error::NotFound)))
    }
}

impl Source for Temper {
    fn name(&self) -> &Name {
        &self.name
    }

    fn origin(&self) -> &str {
        &self.origin
    }

    /// Temperature; humidity too, found on the first read of a stick
    /// that measures it.
    fn quantities(&self) -> Vec<Quantity> {
        vec![Quantity::Temperature]
    }

    fn read(&mut self) -> Result<Vec<Measure>, ReadError> {
        let open = self.opened()?;
        let reading = match open.stick.reading() {
            Ok(reading) => reading,
            Err(e) => {
                self.open = None;
                return Err(stick_error(e));
            }
        };
        let temperature = Measure {
            quantity: Quantity::Temperature,
            value: reading.temperature.get(),
        };
        let humidity = reading.humidity.map(|humidity| Measure {
            quantity: Quantity::Humidity,
            value: humidity.get(),
        });
        Ok(std::iter::once(temperature).chain(humidity).collect())
    }

    fn device(&self) -> Option<String> {
        self.open.as_ref().map(|open| open.firmware.clone())
    }
}

/// A stick's error, and every cause under it, as one line.
fn stick_error(error: impl Error) -> ReadError {
    let mut line = error.to_string();
    let mut cause = error.source();
    while let Some(next) = cause {
        line.push_str(": ");
        line.push_str(&next.to_string());
        cause = next.source();
    }
    ReadError::Stick(line)
}

#[cfg(test)]
mod tests {
    use super::FIRST_FOUND;
    use super::parse;
    use crate::sensor::ConfigError;
    use crate::sensor::Quantity;
    use crate::sensor::Source;
    use std::path::Path;

    #[test]
    fn a_stick_is_named_and_may_be_given_a_path() {
        let first = parse("room").expect("a name alone");
        assert_eq!(first.name().as_str(), "room");
        assert_eq!(first.path, None);
        assert_eq!(first.origin(), FIRST_FOUND);
        let given = parse("bench=/run/temper/bench").expect("a name and a path");
        assert_eq!(given.path.as_deref(), Some(Path::new("/run/temper/bench")));
        assert_eq!(given.origin(), "/run/temper/bench");
        assert_eq!(given.quantities(), vec![Quantity::Temperature]);
        assert!(matches!(parse("Room"), Err(ConfigError::Name(_))));
        assert!(matches!(parse("room="), Err(ConfigError::Form(_))));
    }
}
