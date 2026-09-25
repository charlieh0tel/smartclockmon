//! The Prometheus text exposition format, written by hand.
//!
//! By hand because the format is a dozen lines of rules and the
//! alternative is a dependency with its own registry, its own macros
//! and its own idea of when a metric exists.  What matters here is
//! which metrics are absent: a field the receiver did not answer is
//! left out rather than exported as zero, because zero is a reading and
//! absence is not.
//!
//! One scrape covers every daemon on the host.  Each sample carries
//! the labels that say which: the daemon instance, and the serial and
//! model of the receiver it is attached to.

use std::fmt::Write as _;

use smartclock::snapshot::Freshness;
use smartclock::snapshot::Tier;
use smartclock::wire::Reading;

/// Every metric this exporter emits, prefixed to keep the namespace.
const PREFIX: &str = "smartclock";

/// One daemon's part of a scrape: the labels that name it and what it
/// answered.
pub(crate) struct Scrape {
    /// Already rendered, `daemon="bench",serial="...",model="..."`,
    /// without braces; empty for an exporter pointed at one socket
    /// with nothing known about it.
    pub(crate) labels: String,
    /// The daemon's last reading, or nothing if it could not be asked.
    pub(crate) reading: Option<Reading>,
}

/// One metric's samples across every daemon scraped, in the order the
/// metric was first written.
///
/// Prometheus wants a metric's HELP and TYPE once, ahead of all its
/// samples, so samples from several daemons are gathered by name
/// before anything is written.
struct Family {
    name: &'static str,
    help: &'static str,
    /// Rendered label set and value, one per daemon that had it.
    samples: Vec<(String, f64)>,
}

/// The scrape being assembled, keyed by metric name in first-seen order.
#[derive(Default)]
struct Families(Vec<Family>);

impl Families {
    /// One sample of a gauge under `labels`.
    fn gauge(&mut self, name: &'static str, help: &'static str, labels: &str, value: f64) {
        let family = match self.0.iter_mut().find(|f| f.name == name) {
            Some(family) => family,
            None => {
                self.0.push(Family {
                    name,
                    help,
                    samples: Vec::new(),
                });
                self.0.last_mut().expect("just pushed")
            }
        };
        family.samples.push((labels.to_owned(), value));
    }

    /// The same, for a value the receiver may not have answered.
    ///
    /// `None` emits nothing at all.  Exporting a missing reading as
    /// zero would put a plausible number on a graph, which is the
    /// failure this whole project is written to avoid.
    fn maybe(&mut self, name: &'static str, help: &'static str, labels: &str, value: Option<f64>) {
        if let Some(value) = value {
            self.gauge(name, help, labels, value);
        }
    }

    fn render(&self) -> String {
        let mut out = String::with_capacity(4096);
        for family in &self.0 {
            let _ = writeln!(out, "# HELP {PREFIX}_{} {}", family.name, family.help);
            let _ = writeln!(out, "# TYPE {PREFIX}_{} gauge", family.name);
            for (labels, value) in &family.samples {
                if labels.is_empty() {
                    let _ = writeln!(out, "{PREFIX}_{} {value}", family.name);
                } else {
                    let _ = writeln!(out, "{PREFIX}_{}{{{labels}}} {value}", family.name);
                }
            }
        }
        out
    }
}

/// `labels` with one more `key="value"` on the end.
fn with(labels: &str, key: &str, value: &str) -> String {
    if labels.is_empty() {
        format!("{key}=\"{value}\"")
    } else {
        format!("{labels},{key}=\"{value}\"")
    }
}

/// Render everything a scrape should see, from every daemon.
///
/// `up` says whether a daemon answered at all.  When it did not, that
/// is its only metric: a stale set of numbers with `up 0` beside them
/// invites a panel to keep drawing the numbers.
pub(crate) fn render(scrapes: &[Scrape]) -> String {
    let mut families = Families::default();
    for scrape in scrapes {
        one(&mut families, scrape);
    }
    families.render()
}

/// One daemon's samples.
fn one(out: &mut Families, scrape: &Scrape) {
    let labels = scrape.labels.as_str();
    out.gauge(
        "up",
        "1 when the daemon answered this scrape",
        labels,
        f64::from(u8::from(scrape.reading.is_some())),
    );
    let Some(r) = &scrape.reading else {
        return;
    };

    // Freshness as a number, so an alert can fire on the link being
    // down without parsing a label.
    out.gauge(
        "reading_live",
        "1 when every tier has been polled and none is failing",
        labels,
        f64::from(u8::from(r.freshness == Freshness::Live)),
    );
    out.gauge(
        "reading_disconnected",
        "1 when the link to the receiver is down",
        labels,
        f64::from(u8::from(r.freshness == Freshness::Disconnected)),
    );

    // A value is exported only while the tier that reads it is reading.
    // With the link down or that tier failing, the number is the last
    // one read, and a panel would draw it as current.
    let current =
        |tier: Tier| r.freshness != Freshness::Disconnected && r.polled.get(tier).error.is_none();
    let fast = current(Tier::Fast);
    let medium = current(Tier::Medium);
    let slow = current(Tier::Slow);

    out.maybe(
        "efc_percent",
        "Oscillator control as a share of its range, -100 to 100",
        labels,
        r.efc.filter(|_| fast).map(|v| v.percent()),
    );
    out.maybe(
        "efc_raw",
        "Oscillator control as the raw 20-bit value",
        labels,
        r.efc_raw.filter(|_| medium).map(f64::from),
    );
    out.maybe(
        "temperature_celsius",
        "Internal temperature; not the oscillator oven, which runs far hotter",
        labels,
        r.temperature_c.filter(|_| medium),
    );
    out.maybe(
        "oven_current",
        "Oven current",
        labels,
        r.oven_current.filter(|_| medium),
    );
    out.maybe(
        "oven_tempco",
        "Oscillator temperature coefficient, in parts in 10^12 per degree C; \
         measured against GPS while locked and kept in EPROM, so it sits \
         still for long stretches",
        labels,
        r.oven_tempco.filter(|_| slow),
    );
    out.maybe(
        "time_interval_seconds",
        "Interval between the receiver's 1 PPS and GPS",
        labels,
        r.time_interval_ns.filter(|_| fast).map(|ns| ns * 1e-9),
    );
    out.maybe(
        "tfom",
        "Time figure of merit, lower is better",
        labels,
        r.tfom.filter(|_| fast).map(|v| f64::from(v.get())),
    );
    out.maybe(
        "ffom",
        "Frequency figure of merit, lower is better",
        labels,
        r.ffom.filter(|_| fast).map(|v| f64::from(v.get())),
    );
    out.maybe(
        "hardware_bits",
        "Hardware condition register; 0 is healthy",
        labels,
        r.hardware.filter(|_| fast).map(|h| f64::from(h.bits())),
    );
    out.maybe(
        "alarm",
        "1 while the receiver has something latched in a status group; \
         this is what its front-panel Alarm LED is showing",
        labels,
        r.alarming
            .filter(|_| medium)
            .map(|v| f64::from(u8::from(v))),
    );
    out.maybe(
        "time_reset",
        "1 once the receiver has stepped its own clock to match the satellites, \
         which invalidates interval measurements taken across the step; \
         stays set until the alarm is cleared at the receiver",
        labels,
        r.time_reset
            .filter(|_| medium)
            .map(|v| f64::from(u8::from(v))),
    );
    // The condition registers, as the booleans they decode to.  A
    // register exported as a number would need the manual and a
    // bitwise expression in every alert that used it.
    for (name, help, value, tier) in [
        ("locked", "1 while locked to GPS", r.locked, medium),
        (
            "reference_valid",
            "1 while the GPS 1 PPS is fit to discipline against",
            r.reference_valid,
            medium,
        ),
        (
            "position_hold",
            "1 while holding a surveyed position rather than surveying",
            r.position_hold,
            medium,
        ),
        (
            "log_almost_full",
            "1 when the receiver's diagnostic log is near the point where it stops recording",
            r.log_almost_full,
            medium,
        ),
        (
            "oven_warm",
            "1 once the oscillator oven has warmed up since powerup",
            r.oven_warm,
            slow,
        ),
        (
            "date_time_valid",
            "1 once the date and time were set at the first lock after powerup",
            r.date_time_valid,
            slow,
        ),
        (
            "holdover_recovering",
            "1 while coming out of holdover",
            r.holdover_recovering,
            medium,
        ),
        (
            "holdover_exceeding_threshold",
            "1 while holdover has run past its configured threshold",
            r.holdover_exceeding_threshold,
            medium,
        ),
    ] {
        out.maybe(
            name,
            help,
            labels,
            value.filter(|_| tier).map(|v| f64::from(u8::from(v))),
        );
    }
    out.maybe(
        "holdover_active",
        "1 while the receiver is in holdover",
        labels,
        r.holdover_active
            .filter(|_| medium)
            .map(|v| f64::from(u8::from(v))),
    );
    out.maybe(
        "holdover_seconds",
        "How long the current or last holdover lasted",
        labels,
        r.holdover_seconds.filter(|_| medium),
    );
    out.maybe(
        "holdover_predicted_seconds",
        "Predicted error after 24 hours of holdover",
        labels,
        r.holdover_predicted_s.filter(|_| medium),
    );
    out.maybe(
        "holdover_present_seconds",
        "Error accumulated so far in the current holdover",
        labels,
        r.holdover_present_s.filter(|_| medium),
    );
    out.maybe(
        "rollover_epochs",
        "GPS week epochs the receiver's calendar is behind",
        labels,
        r.date
            .filter(|_| slow)
            .map(|d| f64::from(d.rollover().map_or(0, |s| s.epochs))),
    );

    // From the direct counts on the medium tier, not the status screen:
    // no tier reads the screen, so a count taken from it is whatever the
    // last sky plot saw, however long ago.
    out.maybe(
        "satellites_tracked",
        "Satellites being tracked",
        labels,
        r.tracking.filter(|_| medium).map(f64::from),
    );
    out.maybe(
        "satellites_visible",
        "Satellites the almanac predicts are visible",
        labels,
        r.visible.filter(|_| medium).map(f64::from),
    );

    // The age of each group of fields.  Without these a daemon that has
    // stopped polling looks like a remarkably steady oscillator: the
    // numbers above stay exactly where they were.
    let now = jiff::Timestamp::now();
    for (tier, state) in [
        ("fast", &r.polled.fast),
        ("medium", &r.polled.medium),
        ("slow", &r.polled.slow),
    ] {
        let tiered = with(labels, "tier", tier);
        // Skipped rather than defaulted: an age of zero reads as
        // "polled just now", which is the one answer that must not be
        // invented for a tier whose age is unknown.
        if let Some(age) = state
            .at
            .and_then(|at| (now - at).total(jiff::Unit::Second).ok())
        {
            out.gauge(
                "tier_age_seconds",
                "How long ago this group of fields was last read",
                &tiered,
                age.max(0.0),
            );
        }
        out.gauge(
            "tier_failing",
            "1 when this group's last read failed",
            &tiered,
            f64::from(u8::from(state.error.is_some())),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::Scrape;
    use super::render;
    use smartclock::snapshot::Freshness;
    use smartclock::snapshot::Snapshot;
    use smartclock::snapshot::Tier;
    use smartclock::types::EfcPercent;
    use smartclock::wire::Reading;

    #[test]
    fn an_unreachable_daemon_exports_no_readings_at_all() {
        // Not stale numbers with up 0 beside them: a panel that plots
        // the numbers and ignores the flag then shows a receiver that
        // looks perfectly steady while nothing is being read.
        let out = render(&[Scrape {
            labels: String::new(),
            reading: None,
        }]);
        assert!(out.contains("smartclock_up 0"));
        assert!(
            !out.contains("efc"),
            "a scrape that failed exported a reading"
        );
        assert!(!out.contains("tier_age"));
    }

    /// A snapshot with a value from every tier, every tier polled.
    fn read_everywhere() -> Snapshot {
        let now = jiff::Timestamp::now();
        let mut snapshot = Snapshot::new(now);
        snapshot.efc = EfcPercent::new(1.0);
        snapshot.temperature = Some(35.0);
        snapshot.oven_tempco = Some(0.5);
        for tier in Tier::ALL {
            snapshot.polled.succeeded(tier, now);
        }
        snapshot.settle_freshness();
        snapshot
    }

    #[test]
    fn two_daemons_share_one_help_line_per_metric_and_keep_their_labels() {
        let out = render(&[
            Scrape {
                labels: r#"daemon="a",serial="1""#.to_owned(),
                reading: Some(Reading::from(&read_everywhere())),
            },
            Scrape {
                labels: r#"daemon="b""#.to_owned(),
                reading: None,
            },
        ]);
        assert_eq!(out.matches("# HELP smartclock_up ").count(), 1);
        assert!(out.contains(r#"smartclock_up{daemon="a",serial="1"} 1"#));
        assert!(out.contains(r#"smartclock_up{daemon="b"} 0"#));
        assert!(out.contains(r#"smartclock_efc_percent{daemon="a",serial="1"} 1"#));
        assert!(!out.contains(r#"smartclock_efc_percent{daemon="b"}"#));
        assert!(out.contains(r#"smartclock_tier_failing{daemon="a",serial="1",tier="fast"} 0"#));
        // Every sample of a family sits under its one header.
        let up = out.find("# TYPE smartclock_up gauge").expect("type line");
        let next = out
            .find("# HELP smartclock_reading_live")
            .expect("next family");
        let block = &out[up..next];
        assert_eq!(block.matches("smartclock_up{").count(), 2);
    }

    #[test]
    fn a_disconnected_receiver_exports_no_readings() {
        let mut snapshot = read_everywhere();
        snapshot.freshness = Freshness::Disconnected;
        let out = render(&[Scrape {
            labels: String::new(),
            reading: Some(Reading::from(&snapshot)),
        }]);
        assert!(out.contains("smartclock_reading_disconnected 1"));
        for absent in [
            "smartclock_efc_percent",
            "smartclock_temperature_celsius",
            "smartclock_oven_tempco",
        ] {
            assert!(
                !out.contains(absent),
                "{absent} exported while disconnected"
            );
        }
    }

    #[test]
    fn a_failing_tier_exports_none_of_its_values_and_the_others_still_report() {
        let mut snapshot = read_everywhere();
        snapshot.polled.failed(Tier::Medium, "no answer");
        snapshot.settle_freshness();
        let out = render(&[Scrape {
            labels: String::new(),
            reading: Some(Reading::from(&snapshot)),
        }]);
        assert!(!out.contains("smartclock_temperature_celsius"));
        assert!(out.contains("smartclock_efc_percent 1"));
        assert!(out.contains("smartclock_oven_tempco 0.5"));
    }

    #[test]
    fn a_field_the_receiver_did_not_answer_is_absent_not_zero() {
        // Zero is a reading.  An EFC of zero means the oscillator is
        // centred, which is a very different thing from not having
        // asked, and Grafana cannot tell them apart after the fact.
        let empty = Reading::from(&Snapshot::new(jiff::Timestamp::now()));
        let out = render(&[Scrape {
            labels: String::new(),
            reading: Some(empty),
        }]);
        assert!(out.contains("smartclock_up 1"));
        for absent in [
            "smartclock_efc_percent",
            "smartclock_temperature_celsius",
            "smartclock_tfom",
            "smartclock_satellites_tracked",
            "smartclock_satellites_visible",
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
