//! Sensors as Linux's kernel presents them, through its two interfaces
//! for them, hwmon and IIO, so nothing here knows the part behind one.
//!
//! Only files are read, so this builds anywhere; the command line
//! offers it only where [`PRESENT`].  The units are the kernel's
//! (`Documentation/ABI/testing/sysfs-class-hwmon` and `sysfs-bus-iio`),
//! and a reading is converted to the unit stored: degrees C, percent
//! relative humidity, kilopascals.

use std::path::Path;
use std::path::PathBuf;

use crate::sensor::ConfigError;
use crate::sensor::Measure;
use crate::sensor::Name;
use crate::sensor::Quantity;
use crate::sensor::ReadError;
use crate::sensor::Source;

/// Whether there is a sysfs to read: on Linux, and nowhere else.
pub const PRESENT: bool = cfg!(target_os = "linux");

/// Characters that make a path a glob.
const GLOB: [char; 3] = ['*', '?', '['];

/// What the kernel's value for `quantity` is divided by to give the
/// stored unit: temperature and humidity come in thousandths, pressure
/// in kilopascals already.
fn kernel_per_unit(quantity: Quantity) -> f64 {
    match quantity {
        Quantity::Temperature | Quantity::Humidity => 1000.0,
        Quantity::Pressure => 1.0,
    }
}

/// The IIO channel type that measures `quantity`.
fn iio_type(quantity: Quantity) -> &'static str {
    match quantity {
        Quantity::Temperature => "temp",
        Quantity::Humidity => "humidityrelative",
        Quantity::Pressure => "pressure",
    }
}

/// Which of the kernel's interfaces a sensor is read through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interface {
    /// `/sys/class/hwmon`: one `*_input` file per reading.
    Hwmon,
    /// `/sys/bus/iio`: a channel, read from its `_input`, or from its
    /// `_raw`, `_offset` and `_scale`.
    Iio,
}

/// One configured sysfs sensor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    /// What it is called.
    pub name: Name,
    /// What it measures, from the kernel's file name.
    pub quantity: Quantity,
    /// How it is read.
    pub interface: Interface,
    /// The path as configured: for hwmon a `*_input` file, for IIO a
    /// channel's path less its suffix.  Its directory may be a glob or
    /// go through symlinks; its last component is literal.
    pub source: String,
}

/// Parse a sensor configured as `NAME=PATH` for `interface`.
pub fn parse(interface: Interface, spec: &str) -> Result<Channel, ConfigError> {
    let (name, source) = spec
        .split_once('=')
        .filter(|(_, path)| !path.is_empty())
        .ok_or_else(|| ConfigError::Form(spec.to_owned()))?;
    let name = Name::new(name.trim())?;
    let source = source.trim().to_owned();
    let last = last_component(&source);
    if last.contains(GLOB) {
        return Err(ConfigError::GlobbedLast(source));
    }
    if interface == Interface::Iio && IIO_SUFFIXES.iter().any(|s| last.ends_with(s)) {
        return Err(ConfigError::Suffix(source));
    }
    let quantity = match interface {
        Interface::Hwmon => hwmon_quantity(last),
        Interface::Iio => iio_quantity(last),
    }
    .ok_or_else(|| ConfigError::Kind(source.clone()))?;
    Ok(Channel {
        name,
        quantity,
        interface,
        source,
    })
}

fn last_component(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn directory(path: &str) -> &str {
    path.rsplit_once('/').map_or(".", |(dir, _)| dir)
}

/// What a hwmon file measures: `temp<N>_input` or `humidity<N>_input`.
fn hwmon_quantity(file: &str) -> Option<Quantity> {
    let stem = file.strip_suffix("_input")?;
    let (kind, index) = stem.split_at(stem.find(|c: char| c.is_ascii_digit())?);
    if !index.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    match kind {
        "temp" => Some(Quantity::Temperature),
        "humidity" => Some(Quantity::Humidity),
        _ => None,
    }
}

/// The suffixes a channel's attributes take, which a configured channel
/// must not carry itself.
const IIO_SUFFIXES: [&str; 4] = ["_input", "_raw", "_offset", "_scale"];

/// What an IIO channel measures: `in_<type>[index][_modifier]`, such as
/// `in_temp`, `in_temp0` or `in_temp_ambient`.
fn iio_quantity(channel: &str) -> Option<Quantity> {
    let rest = channel.strip_prefix("in_")?;
    // Longest type first: `humidityrelative` would otherwise be no
    // match at all, and no type is a prefix of another today.
    [
        Quantity::Humidity,
        Quantity::Pressure,
        Quantity::Temperature,
    ]
    .into_iter()
    .find(|q| {
        rest.strip_prefix(iio_type(*q)).is_some_and(|after| {
            let after = after.trim_start_matches(|c: char| c.is_ascii_digit());
            after.is_empty() || after.starts_with('_')
        })
    })
}

/// The directory a sensor's files are in, its glob resolved now.
fn resolve_directory(sensor: &Channel) -> Result<PathBuf, ReadError> {
    let dir = directory(&sensor.source);
    if !dir.contains(GLOB) {
        return Ok(PathBuf::from(dir));
    }
    let matches: Vec<PathBuf> = glob::glob(dir)
        .map_err(|e| ReadError::Pattern(dir.to_owned(), e.to_string()))?
        .filter_map(Result::ok)
        .collect();
    match matches.as_slice() {
        [] => Err(ReadError::Missing(sensor.source.clone())),
        [one] => Ok(one.clone()),
        many => Err(ReadError::Ambiguous {
            pattern: sensor.source.clone(),
            count: many.len(),
        }),
    }
}

/// A sysfs file's number, or `None` where the file does not exist.
fn number(path: &Path) -> Result<Option<f64>, ReadError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ReadError::Io {
                path: path.to_owned(),
                source,
            });
        }
    };
    text.trim()
        .parse()
        .map(Some)
        .map_err(|_| ReadError::NotANumber {
            path: path.to_owned(),
            text: text.trim().to_owned(),
        })
}

/// Read a sensor now, in the unit stored.
fn read(sensor: &Channel) -> Result<f64, ReadError> {
    let dir = resolve_directory(sensor)?;
    let last = last_component(&sensor.source);
    let missing = || ReadError::Missing(sensor.source.clone());
    let kernel = match sensor.interface {
        Interface::Hwmon => number(&dir.join(last))?.ok_or_else(missing)?,
        Interface::Iio => {
            let attribute = |suffix: &str| number(&dir.join(format!("{last}{suffix}")));
            // Shared by every channel of the type when the channel has
            // none of its own: `in_temp_scale` for `in_temp0`.
            let shared = |suffix: &str| -> Result<Option<f64>, ReadError> {
                match attribute(suffix)? {
                    Some(value) => Ok(Some(value)),
                    None => number(&dir.join(format!("in_{}{suffix}", iio_type(sensor.quantity)))),
                }
            };
            match attribute("_input")? {
                Some(input) => input,
                None => {
                    let raw = attribute("_raw")?.ok_or_else(missing)?;
                    let offset = shared("_offset")?.unwrap_or(0.0);
                    let scale = shared("_scale")?.ok_or_else(missing)?;
                    (raw + offset) * scale
                }
            }
        }
    };
    Ok(kernel / kernel_per_unit(sensor.quantity))
}

/// The kernel's name for the device a sensor is on, as its `name`
/// attribute gives it, if it can be read now.
fn device(sensor: &Channel) -> Option<String> {
    let dir = resolve_directory(sensor).ok()?;
    let name = std::fs::read_to_string(dir.join("name")).ok()?;
    Some(name.trim().to_owned())
}

impl Source for Channel {
    fn name(&self) -> &Name {
        &self.name
    }

    fn origin(&self) -> &str {
        &self.source
    }

    fn quantities(&self) -> Vec<Quantity> {
        vec![self.quantity]
    }

    fn read(&mut self) -> Result<Vec<Measure>, ReadError> {
        Ok(vec![Measure {
            quantity: self.quantity,
            value: read(self)?,
        }])
    }

    fn device(&self) -> Option<String> {
        device(self)
    }
}

#[cfg(test)]
mod tests {
    use super::Interface;
    use super::device;
    use super::parse;
    use super::read;
    use crate::sensor::ConfigError;
    use crate::sensor::Quantity;
    use crate::sensor::ReadError;
    use crate::sensor::Source;
    use crate::sensor::check_distinct;
    use std::path::PathBuf;

    /// A directory standing in for sysfs, removed when this goes.
    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("smartclock-sensord-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("a root");
            Self(root)
        }

        /// Write `text` to `path` under the root, making its directory.
        fn file(&self, path: &str, text: &str) -> &Self {
            let path = self.0.join(path);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
            std::fs::write(path, format!("{text}\n")).expect("a file");
            self
        }

        fn path(&self, path: &str) -> String {
            self.0.join(path).display().to_string()
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn a_sensor_is_a_name_and_a_path() {
        assert!(parse(Interface::Hwmon, "room=/x/temp1_input").is_ok());
        assert_eq!(
            parse(Interface::Hwmon, "Room=/x/temp1_input"),
            Err(ConfigError::Name("Room".to_owned()))
        );
        assert_eq!(
            parse(Interface::Hwmon, "room"),
            Err(ConfigError::Form("room".to_owned()))
        );
    }

    #[test]
    fn the_quantity_comes_from_the_file_name() {
        let quantity =
            |interface, path: &str| parse(interface, &format!("s={path}")).map(|s| s.quantity);
        assert_eq!(
            quantity(Interface::Hwmon, "/h/temp1_input"),
            Ok(Quantity::Temperature)
        );
        assert_eq!(
            quantity(Interface::Hwmon, "/h/humidity1_input"),
            Ok(Quantity::Humidity)
        );
        assert_eq!(
            quantity(Interface::Iio, "/d/in_temp"),
            Ok(Quantity::Temperature)
        );
        assert_eq!(
            quantity(Interface::Iio, "/d/in_temp0"),
            Ok(Quantity::Temperature)
        );
        assert_eq!(
            quantity(Interface::Iio, "/d/in_temp_ambient"),
            Ok(Quantity::Temperature)
        );
        assert_eq!(
            quantity(Interface::Iio, "/d/in_humidityrelative"),
            Ok(Quantity::Humidity)
        );
        assert_eq!(
            quantity(Interface::Iio, "/d/in_pressure0"),
            Ok(Quantity::Pressure)
        );
        for bad in [
            (Interface::Hwmon, "/h/fan1_input"),
            (Interface::Hwmon, "/h/temp1_max"),
            (Interface::Iio, "/d/in_voltage0"),
            (Interface::Iio, "/d/in_tempx"),
        ] {
            assert!(
                matches!(quantity(bad.0, bad.1), Err(ConfigError::Kind(_))),
                "{bad:?}"
            );
        }
        assert!(matches!(
            quantity(Interface::Hwmon, "/h/temp*_input"),
            Err(ConfigError::GlobbedLast(_))
        ));
        assert!(matches!(
            quantity(Interface::Iio, "/d/in_temp0_input"),
            Err(ConfigError::Suffix(_))
        ));
    }

    #[test]
    fn one_name_may_measure_two_quantities_but_not_one_twice() {
        let channel = |interface, spec| -> Box<dyn Source> {
            Box::new(parse(interface, spec).expect("parse"))
        };
        let both = [
            channel(Interface::Hwmon, "room=/h/temp1_input"),
            channel(Interface::Hwmon, "room=/h/humidity1_input"),
        ];
        assert_eq!(check_distinct(&both), Ok(()));
        let twice = [
            channel(Interface::Hwmon, "room=/h/temp1_input"),
            channel(Interface::Iio, "room=/d/in_temp"),
        ];
        assert!(matches!(
            check_distinct(&twice),
            Err(ConfigError::Repeated { .. })
        ));
    }

    #[test]
    fn hwmon_is_read_in_thousandths() {
        let tree = Tree::new("hwmon");
        tree.file("hwmon3/temp1_input", "23125")
            .file("hwmon3/humidity1_input", "41500")
            .file("hwmon3/name", "sht4x");
        let temperature = parse(
            Interface::Hwmon,
            &format!("room={}", tree.path("hwmon3/temp1_input")),
        )
        .expect("parse");
        assert!(close(read(&temperature).expect("read"), 23.125));
        let humidity = parse(
            Interface::Hwmon,
            &format!("room={}", tree.path("hwmon3/humidity1_input")),
        )
        .expect("parse");
        assert!(close(read(&humidity).expect("read"), 41.5));
        assert_eq!(device(&temperature).as_deref(), Some("sht4x"));
    }

    #[test]
    fn iio_prefers_input_then_raw_offset_and_scale_falling_back_to_the_type() {
        let tree = Tree::new("iio");
        let sensor = |channel: &str| {
            parse(
                Interface::Iio,
                &format!("bench={}", tree.path(&format!("iio:device0/{channel}"))),
            )
            .expect("parse")
        };
        // Processed.
        tree.file("iio:device0/in_temp_input", "22500");
        assert!(close(read(&sensor("in_temp")).expect("read"), 22.5));
        // Raw with the channel's own scale and offset.
        tree.file("iio:device0/in_temp0_raw", "2880")
            .file("iio:device0/in_temp0_scale", "7.8125")
            .file("iio:device0/in_temp0_offset", "10");
        assert!(close(
            read(&sensor("in_temp0")).expect("read"),
            2.89 * 7.8125
        ));
        // Raw with the type's scale and no offset.
        tree.file("iio:device0/in_temp1_raw", "100")
            .file("iio:device0/in_temp_scale", "250");
        assert!(close(read(&sensor("in_temp1")).expect("read"), 25.0));
        // Pressure is in kilopascals already.
        tree.file("iio:device0/in_pressure_input", "101.325");
        assert!(close(read(&sensor("in_pressure")).expect("read"), 101.325));
        // Raw without a scale is not a reading.
        tree.file("iio:device0/in_humidityrelative_raw", "5");
        assert!(matches!(
            read(&sensor("in_humidityrelative")),
            Err(ReadError::Missing(_))
        ));
    }

    #[test]
    fn a_glob_must_match_one_directory_and_a_missing_file_is_a_missed_reading() {
        let tree = Tree::new("glob");
        tree.file("i2c/1-0044/hwmon/hwmon4/temp1_input", "21000");
        let globbed = parse(
            Interface::Hwmon,
            &format!("room={}", tree.path("i2c/*-0044/hwmon/hwmon*/temp1_input")),
        )
        .expect("parse");
        assert!(close(read(&globbed).expect("read"), 21.0));
        tree.file("i2c/2-0044/hwmon/hwmon5/temp1_input", "22000");
        assert!(matches!(
            read(&globbed),
            Err(ReadError::Ambiguous { count: 2, .. })
        ));
        let gone = parse(
            Interface::Hwmon,
            &format!("room={}", tree.path("nowhere/temp1_input")),
        )
        .expect("parse");
        assert!(matches!(read(&gone), Err(ReadError::Missing(_))));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_directory_is_followed_on_every_read() {
        let tree = Tree::new("link");
        tree.file("a/temp1_input", "20000")
            .file("b/temp1_input", "30000");
        let link = PathBuf::from(tree.path("room"));
        let link = link.as_path();
        std::os::unix::fs::symlink(tree.path("a"), link).expect("a link");
        let sensor = parse(
            Interface::Hwmon,
            &format!("room={}/temp1_input", link.display()),
        )
        .expect("parse");
        assert!(close(read(&sensor).expect("read"), 20.0));
        std::fs::remove_file(link).expect("unlink");
        std::os::unix::fs::symlink(tree.path("b"), link).expect("relink");
        assert!(close(read(&sensor).expect("read"), 30.0));
    }
}
