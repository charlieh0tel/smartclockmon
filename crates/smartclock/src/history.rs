//! What the readers of the daemon's log agree on.
//!
//! The daemon writes the log and two programs read it, the web view and
//! the monitor.  Neither reader links the daemon, so what they must
//! agree with it about lives here: where it records its cadence, and
//! when a slower tier's value is still a measurement.

use std::time::Duration;

use crate::snapshot::Tier;
use crate::task::Cadence;

/// How many of its tier's intervals a value stays current for.
///
/// Rows are the fast tier's, and a slower tier's value is carried in
/// every one of them until that tier reads again.  Past this many of
/// its intervals without a read the tier is failing, and the value
/// carried is not a measurement.
pub const STALE_AFTER_INTERVALS: f64 = 3.0;

/// The `meta` key the daemon records a tier's cadence under, in
/// seconds.
pub fn cadence_key(tier: Tier) -> String {
    format!("cadence_{}", tier.name())
}

/// The cadence as recorded, from a lookup of [`cadence_key`] values.
///
/// The default for any tier not recorded: a log written before the
/// cadence was recorded ran at the defaults of its day or at whatever
/// flags it was given, and the defaults are the likelier.
pub fn recorded_cadence(lookup: impl Fn(&str) -> Option<String>) -> Cadence {
    let default = Cadence::default();
    let recorded = |tier: Tier| {
        let seconds: f64 = lookup(&cadence_key(tier))?.parse().ok()?;
        Duration::try_from_secs_f64(seconds).ok()
    };
    Cadence {
        fast: recorded(Tier::Fast).unwrap_or(default.fast),
        medium: recorded(Tier::Medium).unwrap_or(default.medium),
        slow: recorded(Tier::Slow).unwrap_or(default.slow),
    }
}

/// A snapshot column as an SQL expression that is null where its
/// value is not current.
///
/// `column` is interpolated and must be a known column name, never
/// caller input.  Rows written before the tier timestamps existed have
/// no `fast_at` and are taken as they are.
pub fn current(column: &str, tier: Tier, cadence: &Cadence) -> String {
    if tier == Tier::Fast {
        return column.to_owned();
    }
    let window = cadence.of(tier).as_secs_f64() * STALE_AFTER_INTERVALS;
    let read = tier.name();
    format!(
        "CASE WHEN fast_at IS NULL
                OR unixepoch({read}_at, 'subsec') >= unixepoch(at, 'subsec') - {window}
              THEN {column} END"
    )
}
