//! The Prometheus text exposition format, written by hand.
//!
//! By hand because the format is a dozen lines of rules and the
//! alternative is a dependency with its own registry, its own macros
//! and its own idea of when a metric exists.  What matters here is
//! which metrics are absent: a field the receiver did not answer is
//! left out rather than exported as zero, because zero is a reading and
//! absence is not.

use std::fmt::Write as _;

use smartclock::snapshot::Freshness;
use smartclock::wire::Reading;

/// Every metric this exporter emits, prefixed to keep the namespace.
const PREFIX: &str = "smartclock";

/// One gauge, with the help and type lines Prometheus wants.
fn gauge(out: &mut String, name: &str, help: &str, value: f64) {
    let _ = writeln!(out, "# HELP {PREFIX}_{name} {help}");
    let _ = writeln!(out, "# TYPE {PREFIX}_{name} gauge");
    let _ = writeln!(out, "{PREFIX}_{name} {value}");
}

/// The same, for a value the receiver may not have answered.
///
/// `None` emits nothing at all.  Exporting a missing reading as zero
/// would put a plausible number on a graph, which is the failure this
/// whole project is written to avoid.
fn maybe(out: &mut String, name: &str, help: &str, value: Option<f64>) {
    if let Some(value) = value {
        gauge(out, name, help, value);
    }
}

/// Render everything a scrape should see.
///
/// `up` says whether the daemon answered at all.  When it did not, that
/// is the only metric: a stale set of numbers with `up 0` beside them
/// invites a panel to keep drawing the numbers.
pub(crate) fn render(reading: Option<&Reading>) -> String {
    let mut out = String::with_capacity(2048);
    gauge(
        &mut out,
        "up",
        "1 when the daemon answered this scrape",
        f64::from(u8::from(reading.is_some())),
    );
    let Some(r) = reading else {
        return out;
    };

    // Freshness as a number, so an alert can fire on the link being
    // down without parsing a label.
    gauge(
        &mut out,
        "reading_live",
        "1 when every tier has been polled and none is failing",
        f64::from(u8::from(r.freshness == Freshness::Live)),
    );
    gauge(
        &mut out,
        "reading_disconnected",
        "1 when the link to the receiver is down",
        f64::from(u8::from(r.freshness == Freshness::Disconnected)),
    );

    maybe(
        &mut out,
        "efc_percent",
        "Oscillator control as a share of its range, -100 to 100",
        r.efc.map(|v| v.percent()),
    );
    maybe(
        &mut out,
        "efc_raw",
        "Oscillator control as the raw 20-bit value",
        r.efc_raw.map(f64::from),
    );
    maybe(
        &mut out,
        "temperature_celsius",
        "Internal temperature; not the oscillator oven, which runs far hotter",
        r.temperature_c,
    );
    maybe(&mut out, "oven_current", "Oven current", r.oven_current);
    maybe(
        &mut out,
        "oven_tempco",
        "Oscillator temperature coefficient as the receiver has learned it; \
         units undocumented, so watch the trend and not the value",
        r.oven_tempco,
    );
    maybe(
        &mut out,
        "time_interval_seconds",
        "Interval between the receiver's 1 PPS and GPS",
        r.time_interval_ns.map(|ns| ns * 1e-9),
    );
    maybe(
        &mut out,
        "tfom",
        "Time figure of merit, lower is better",
        r.tfom.map(|v| f64::from(v.get())),
    );
    maybe(
        &mut out,
        "ffom",
        "Frequency figure of merit, lower is better",
        r.ffom.map(|v| f64::from(v.get())),
    );
    maybe(
        &mut out,
        "hardware_bits",
        "Hardware condition register; 0 is healthy",
        r.hardware.map(|h| f64::from(h.bits())),
    );
    // The condition registers, as the booleans they decode to.  A
    // register exported as a number would need the manual and a
    // bitwise expression in every alert that used it.
    for (name, help, value) in [
        ("locked", "1 while locked to GPS", r.locked),
        (
            "reference_valid",
            "1 while the GPS 1 PPS is fit to discipline against",
            r.reference_valid,
        ),
        (
            "position_hold",
            "1 while holding a surveyed position rather than surveying",
            r.position_hold,
        ),
        (
            "log_almost_full",
            "1 when the receiver's diagnostic log is near the point where it stops recording",
            r.log_almost_full,
        ),
        (
            "oven_warm",
            "1 once the oscillator oven has warmed up since powerup",
            r.oven_warm,
        ),
        (
            "date_time_valid",
            "1 once the date and time were set at the first lock after powerup",
            r.date_time_valid,
        ),
        (
            "holdover_recovering",
            "1 while coming out of holdover",
            r.holdover_recovering,
        ),
        (
            "holdover_exceeding_threshold",
            "1 while holdover has run past its configured threshold",
            r.holdover_exceeding_threshold,
        ),
    ] {
        maybe(&mut out, name, help, value.map(|v| f64::from(u8::from(v))));
    }
    maybe(
        &mut out,
        "holdover_active",
        "1 while the receiver is in holdover",
        r.holdover_active.map(|v| f64::from(u8::from(v))),
    );
    maybe(
        &mut out,
        "holdover_seconds",
        "How long the current or last holdover lasted",
        r.holdover_seconds,
    );
    maybe(
        &mut out,
        "holdover_predicted_seconds",
        "Predicted error after 24 hours of holdover",
        r.holdover_predicted_s,
    );
    maybe(
        &mut out,
        "holdover_present_seconds",
        "Error accumulated so far in the current holdover",
        r.holdover_present_s,
    );
    maybe(
        &mut out,
        "rollover_epochs",
        "GPS week epochs the receiver's calendar is behind",
        r.date
            .map(|d| f64::from(d.rollover().map_or(0, |s| s.epochs))),
    );

    if let Some(screen) = r.screen.as_ref() {
        maybe(
            &mut out,
            "satellites_tracked",
            "Satellites being tracked",
            screen.tracking.map(f64::from),
        );
        gauge(
            &mut out,
            "satellites_visible",
            "Satellites in the receiver's table, tracked or not",
            screen.satellites.len() as f64,
        );
        gauge(
            &mut out,
            "satellites_suspect",
            "1 when the satellite table disagrees with its own counts",
            f64::from(u8::from(screen.satellites_suspect)),
        );
    }

    // The age of each group of fields.  Without these a daemon that has
    // stopped polling looks like a remarkably steady oscillator: the
    // numbers above stay exactly where they were.
    let _ = writeln!(
        out,
        "# HELP {PREFIX}_tier_age_seconds How long ago this group of fields was last read"
    );
    let _ = writeln!(out, "# TYPE {PREFIX}_tier_age_seconds gauge");
    let now = jiff::Timestamp::now();
    for (tier, state) in [
        ("fast", &r.polled.fast),
        ("medium", &r.polled.medium),
        ("slow", &r.polled.slow),
    ] {
        // Skipped rather than defaulted: an age of zero reads as
        // "polled just now", which is the one answer that must not be
        // invented for a tier whose age is unknown.
        if let Some(age) = state
            .at
            .and_then(|at| (now - at).total(jiff::Unit::Second).ok())
        {
            let _ = writeln!(
                out,
                "{PREFIX}_tier_age_seconds{{tier=\"{tier}\"}} {}",
                age.max(0.0)
            );
        }
    }

    let _ = writeln!(
        out,
        "# HELP {PREFIX}_tier_failing 1 when this group's last read failed"
    );
    let _ = writeln!(out, "# TYPE {PREFIX}_tier_failing gauge");
    for (tier, state) in [
        ("fast", &r.polled.fast),
        ("medium", &r.polled.medium),
        ("slow", &r.polled.slow),
    ] {
        let failing = u8::from(state.error.is_some());
        let _ = writeln!(out, "{PREFIX}_tier_failing{{tier=\"{tier}\"}} {failing}");
    }

    out
}

#[cfg(test)]
mod tests {
    use super::render;
    use smartclock::snapshot::Snapshot;
    use smartclock::wire::Reading;

    #[test]
    fn an_unreachable_daemon_exports_no_readings_at_all() {
        // Not stale numbers with up 0 beside them: a panel that plots
        // the numbers and ignores the flag then shows a receiver that
        // looks perfectly steady while nothing is being read.
        let out = render(None);
        assert!(out.contains("smartclock_up 0"));
        assert!(
            !out.contains("efc"),
            "a scrape that failed exported a reading"
        );
        assert!(!out.contains("tier_age"));
    }

    #[test]
    fn a_field_the_receiver_did_not_answer_is_absent_not_zero() {
        // Zero is a reading.  An EFC of zero means the oscillator is
        // centred, which is a very different thing from not having
        // asked, and Grafana cannot tell them apart after the fact.
        let empty = Reading::from(&Snapshot::new(jiff::Timestamp::now()));
        let out = render(Some(&empty));
        assert!(out.contains("smartclock_up 1"));
        for absent in [
            "smartclock_efc_percent",
            "smartclock_temperature_celsius",
            "smartclock_tfom",
            "smartclock_satellites_tracked",
        ] {
            assert!(
                !out.contains(absent),
                "{absent} was exported without a reading"
            );
        }
        // The tiers still report, since "never read" is itself a fact.
        assert!(out.contains(r#"smartclock_tier_failing{tier="fast"}"#));
    }
}
