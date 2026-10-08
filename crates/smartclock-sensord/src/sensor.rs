//! Sensors, whatever reads them: what one is called, what it measures,
//! and the [`Source`] each kind of sensor is read through.

use std::collections::HashSet;
use std::fmt::Display;
use std::fmt::Formatter;
use std::path::PathBuf;

/// The longest a sensor's name may be.
const NAME_LONGEST: usize = 32;

/// What a sensor measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Quantity {
    /// Degrees C.
    Temperature,
    /// Percent relative humidity.
    Humidity,
    /// Kilopascals.
    Pressure,
}

impl Quantity {
    /// How the log and the readers name it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Temperature => "temperature",
            Self::Humidity => "humidity",
            Self::Pressure => "pressure",
        }
    }

    /// The unit a reading is stored in.
    pub fn unit(self) -> &'static str {
        match self {
            Self::Temperature => "C",
            Self::Humidity => "%RH",
            Self::Pressure => "kPa",
        }
    }
}

/// A sensor's name: a lowercase identifier, since it appears in charts,
/// addresses and metric labels.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Name(String);

impl Name {
    /// The name, if it is one: `[a-z][a-z0-9_]{0,31}`.
    pub fn new(name: &str) -> Result<Self, ConfigError> {
        let mut chars = name.chars();
        let first_ok = chars.next().is_some_and(|c| c.is_ascii_lowercase());
        let rest_ok = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if first_ok && rest_ok && name.len() <= NAME_LONGEST {
            Ok(Self(name.to_owned()))
        } else {
            Err(ConfigError::Name(name.to_owned()))
        }
    }

    /// The name as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for Name {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One reading of one quantity, in the unit stored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measure {
    /// What was measured.
    pub quantity: Quantity,
    /// Its value.
    pub value: f64,
}

/// Something read once a pass, giving a reading of each quantity it
/// measures: a sysfs file, or a device that answers for several.
pub trait Source: Send {
    /// What its readings are called.
    fn name(&self) -> &Name;

    /// Where it is read from, as configured.
    fn origin(&self) -> &str;

    /// What it is known to measure before it is first read.  It may turn
    /// out to measure more.
    fn quantities(&self) -> Vec<Quantity>;

    /// Read it now.
    fn read(&mut self) -> Result<Vec<Measure>, ReadError>;

    /// The part behind it, if it can say now.  Logged with the origin, so
    /// a part swapped behind the same one shows.
    fn device(&self) -> Option<String>;
}

/// Why a sensor's configuration was refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    /// Not `NAME=PATH`.
    #[error("{0:?} is not NAME=PATH")]
    Form(String),
    /// A name that is not a lowercase identifier.
    #[error(
        "{0:?} is not a sensor name: a lowercase letter, then up to 31 lowercase letters, digits or underscores"
    )]
    Name(String),
    /// A last component that is a glob, which would hide the quantity.
    #[error("{0}: the last part of the path must be literal, since it says what is measured")]
    GlobbedLast(String),
    /// An IIO channel given with one of its attributes' suffixes.
    #[error("{0}: name the channel without its _input, _raw, _offset or _scale")]
    Suffix(String),
    /// A file of a kind this does not read.
    #[error("{0}: not a temperature, humidity or pressure reading this can read")]
    Kind(String),
    /// The same name and quantity configured twice.
    #[error("{name} is configured twice as {quantity}")]
    Repeated {
        /// The name.
        name: String,
        /// What both measure.
        quantity: &'static str,
    },
}

/// Why a reading failed.  A failed read is a reading missed, never a
/// reason to stop.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    /// Nothing at the path, or no file the glob matches.
    #[error("{0}: nothing there")]
    Missing(String),
    /// A glob matching more than one file, which would be a guess.
    #[error("{pattern}: {count} files match, not one")]
    Ambiguous {
        /// The glob.
        pattern: String,
        /// How many matched.
        count: usize,
    },
    /// A glob that is not one.
    #[error("{0}: {1}")]
    Pattern(String, String),
    /// The file could not be read.
    #[error("{}: {source}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// The file did not hold a number.
    #[error("{}: {text:?} is not a number", path.display())]
    NotANumber {
        /// The file.
        path: PathBuf,
        /// What it held.
        text: String,
    },
    /// A device could not be found, opened or read, with every cause.
    #[error("{0}")]
    Stick(String),
}

/// Refuse the same name configured twice for one quantity.  One name
/// for two quantities is allowed: a part reading both temperature and
/// humidity is one sensor to its owner.
pub fn check_distinct(sources: &[Box<dyn Source>]) -> Result<(), ConfigError> {
    let mut seen = HashSet::new();
    for source in sources {
        for quantity in source.quantities() {
            if !seen.insert((source.name().clone(), quantity)) {
                return Err(ConfigError::Repeated {
                    name: source.name().to_string(),
                    quantity: quantity.name(),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ConfigError;
    use super::Name;

    #[test]
    fn a_name_is_a_lowercase_identifier() {
        assert!(Name::new("room_2").is_ok());
        for bad in ["Room", "1room", "room-1", "", &"r".repeat(33)] {
            assert_eq!(Name::new(bad), Err(ConfigError::Name(bad.to_owned())));
        }
    }
}
