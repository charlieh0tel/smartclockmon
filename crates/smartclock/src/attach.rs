//! Opening a receiver whose line settings may not be the ones configured.
//!
//! A port's rate and framing are the unit's, not the port's, so a unit
//! that turns up on another port -- two USB adapters enumerated in the
//! other order after a power cut -- answers nothing at the settings its
//! port was given.  The configured settings are tried first, exactly as
//! a plain open would, and only when they get no identity are the
//! settings these receivers are found at tried in turn: the Z3801A's
//! port is fixed at 19200 7O1 (097-z3801-01, 1-8 and 2-10), and the
//! 58503A's is 8N1 at 19200 or the factory 9600 (097-59551-02, 5-101).
//!
//! A probe at the wrong settings reaches the receiver as garbled bytes,
//! which it keeps as errors.  They are read off its queue, at the
//! settings that work, before it is asked anything else -- see
//! [`Session::discard_errors`] for why not `*CLS`.

use crate::device::Device;
use crate::error::Result;
use crate::session::Config;
use crate::session::Session;
use crate::transport;
use crate::transport::Transport;
use crate::transport::serial::Settings;
use crate::types::BaudRate;
use crate::types::Framing;

/// The line settings tried after the configured ones, in order.
pub const FALLBACKS: [(BaudRate, Framing); 4] = [
    (BaudRate::B19200, Framing::EightNone),
    (BaudRate::B19200, Framing::SevenOdd),
    (BaudRate::B9600, Framing::EightNone),
    (BaudRate::B9600, Framing::SevenOdd),
];

/// A receiver opened and identified, and how it was reached.
#[derive(Debug)]
pub struct Attached<T: Transport> {
    /// The receiver.
    pub device: Device<T>,
    /// The rate it answered at.
    pub baud: BaudRate,
    /// The framing it answered at.
    pub framing: Framing,
    /// Whether those are other than the configured settings.
    pub probed: bool,
    /// Entries read off its error queue after probing.  The probes'
    /// garbage is among them, and so is anything the receiver held
    /// already, which cannot be told apart from it.
    pub discarded: usize,
}

impl<T: Transport> Attached<T> {
    /// What the probe did, for a log, when it was needed: `None` when
    /// the configured settings answered.
    pub fn probe_report(&self, configured: &Settings) -> Option<String> {
        self.probed.then(|| {
            format!(
                "{} did not answer at {} {}; found it at {} {}, \
                 and read {} entries off its error queue left by the probe",
                configured.path,
                configured.baud,
                configured.framing,
                self.baud,
                self.framing,
                self.discarded
            )
        })
    }
}

/// Open and identify the receiver at `settings`, or at the first of
/// [`FALLBACKS`] it answers at.
pub fn attach(settings: &Settings, config: &Config) -> Result<Attached<Box<dyn Transport + Send>>> {
    attach_with(settings, config, transport::open)
}

/// [`attach`], opening each port with `open`.
///
/// A port that will not open fails at once rather than being tried at
/// other settings: a missing device or a refused permission is not a
/// framing problem.  If no settings get an identity, the error is the
/// one the configured settings met, which is the one worth reading.
pub fn attach_with<T, F>(settings: &Settings, config: &Config, mut open: F) -> Result<Attached<T>>
where
    T: Transport,
    F: FnMut(&Settings) -> Result<T>,
{
    let mut first_error = None;
    for (n, (baud, framing)) in candidates(settings).into_iter().enumerate() {
        let tried = Settings {
            baud,
            framing,
            ..settings.clone()
        };
        let probed = n > 0;
        let session = Session::new(open(&tried)?, config.clone());
        match identify(session, probed) {
            Ok((device, discarded)) => {
                return Ok(Attached {
                    device,
                    baud,
                    framing,
                    probed,
                    discarded,
                });
            }
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    Err(first_error.expect("the configured settings are always tried"))
}

/// Identify the receiver on `session`, emptying its error queue first
/// when earlier settings have been tried.
fn identify<T: Transport>(mut session: Session<T>, probed: bool) -> Result<(Device<T>, usize)> {
    let discarded = if probed {
        session.sync()?;
        session.discard_errors()?
    } else {
        0
    };
    Ok((Device::open(session)?, discarded))
}

/// The settings to try: the configured ones, then the fallbacks not
/// already tried.  Only the configured ones for a receiver on the
/// network, where the line settings are the bridge's business.
fn candidates(settings: &Settings) -> Vec<(BaudRate, Framing)> {
    let configured = (settings.baud, settings.framing);
    if transport::is_network(&settings.path) {
        return vec![configured];
    }
    std::iter::once(configured)
        .chain(FALLBACKS.into_iter().filter(|c| *c != configured))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::FALLBACKS;
    use super::candidates;
    use crate::transport::serial::Settings;
    use crate::types::BaudRate;
    use crate::types::Framing;

    #[test]
    fn the_configured_settings_come_first_and_are_not_tried_twice() {
        let settings = Settings {
            baud: BaudRate::B19200,
            framing: Framing::SevenOdd,
            ..Settings::default()
        };
        let tried = candidates(&settings);
        assert_eq!(tried[0], (BaudRate::B19200, Framing::SevenOdd));
        assert_eq!(tried.len(), FALLBACKS.len());
        assert_eq!(
            tried.iter().filter(|c| **c == tried[0]).count(),
            1,
            "{tried:?}"
        );
    }

    #[test]
    fn unusual_configured_settings_are_kept_ahead_of_every_fallback() {
        let settings = Settings {
            baud: BaudRate::B2400,
            ..Settings::default()
        };
        let tried = candidates(&settings);
        assert_eq!(tried[0].0, BaudRate::B2400);
        assert_eq!(tried.len(), FALLBACKS.len() + 1);
    }

    #[test]
    fn a_receiver_on_the_network_is_not_probed() {
        let settings = Settings {
            path: "tcp://127.0.0.1:5025".to_owned(),
            ..Settings::default()
        };
        assert_eq!(candidates(&settings).len(), 1);
    }
}
