//! The overlapping Allan deviation against published values and an
//! independent implementation.
//!
//! The estimator's own tests check what it does with gaps, holds and
//! segments, which neither reference has an opinion on; these check
//! that on a plain gapless record the number it produces is the
//! standard one.

use jiff::SignedDuration;
use jiff::Timestamp;
use smartclock::adev::Curve;
use smartclock::adev::Sample;

/// `(tau, deviation, differences)`: allantools 2024.06's `oadev` on
/// [`record`], from `adev_reference.py` beside this file.
const ALLANTOOLS: [(f64, f64, usize); 8] = [
    (1.0, 2.922474329378955e-10, 998),
    (2.0, 2.0111385842605582e-10, 996),
    (5.0, 1.332465067153262e-10, 990),
    (10.0, 9.160140701820414e-11, 980),
    (20.0, 5.3725786047544133e-11, 960),
    (50.0, 3.949276291396878e-11, 900),
    (100.0, 3.23825180387515e-11, 800),
    (200.0, 1.64583837834509e-11, 600),
];

/// Agreement expected between two double-precision implementations of
/// the same sum.
const RELATIVE_TOLERANCE: f64 = 1e-9;

/// The phase record `adev_reference.py` builds, one reading a second.
fn record() -> Vec<Sample> {
    const READINGS: i64 = 1000;
    let start = Timestamp::from_second(1_700_000_000).expect("a timestamp");
    let mut n: u64 = 1_234_567_890;
    let mut phase = 0.0;
    (0..READINGS)
        .map(|i| {
            if i > 0 {
                n = (16_807 * n) % 2_147_483_647;
                phase += (n as f64 / 2_147_483_647.0 - 0.5) * 1e-9;
            }
            Sample {
                at: start + SignedDuration::from_secs(i),
                interval: phase,
            }
        })
        .collect()
}

#[test]
fn a_gapless_record_agrees_with_allantools() {
    let curve = Curve::measure(&record(), 1, |_, _| false);
    let got: Vec<(f64, f64, usize)> = curve
        .points
        .iter()
        .map(|p| (p.tau, p.deviation, p.differences))
        .collect();
    assert_eq!(got.len(), ALLANTOOLS.len(), "{got:?}");
    for ((tau, deviation, differences), (want_tau, want, want_differences)) in
        got.into_iter().zip(ALLANTOOLS)
    {
        assert!(
            (tau - want_tau).abs() < 1e-9,
            "tau {tau} against {want_tau}"
        );
        assert_eq!(differences, want_differences, "differences at tau {tau}");
        let error = (deviation - want).abs() / want;
        assert!(
            error < RELATIVE_TOLERANCE,
            "tau {tau}: {deviation} against {want}, relative error {error}"
        );
    }
}

/// `(tau, overlapping Allan deviation)` for the 1000-point test data
/// set, from NIST SP 1065 (`third_party/NIST-SP-1065.pdf`) Table 31.
const NIST_SP_1065: [(f64, f64); 3] = [
    (1.0, 2.922319e-01),
    (10.0, 9.159953e-02),
    (100.0, 3.241343e-02),
];

/// Table 31 gives seven significant figures.
const PUBLISHED_TOLERANCE: f64 = 5e-7;

/// The SP 1065 section 12.4 data set as phase: 1000 frequency values
/// `n_i / (2^31 - 1)` from `n_(i+1) = 16807 n_i mod (2^31 - 1)`, seeded
/// with `n_0` = 1 234 567 890 and starting at `n_0`, summed at a tau of
/// one second into 1001 phase points.
fn sp_1065() -> Vec<Sample> {
    const MODULUS: u64 = 2_147_483_647;
    let start = Timestamp::from_second(1_700_000_000).expect("a timestamp");
    let mut n: u64 = 1_234_567_890;
    let mut phase = 0.0;
    let mut samples = vec![Sample {
        at: start,
        interval: phase,
    }];
    for i in 1..=1000 {
        phase += n as f64 / MODULUS as f64;
        n = (16_807 * n) % MODULUS;
        samples.push(Sample {
            at: start + SignedDuration::from_secs(i),
            interval: phase,
        });
    }
    samples
}

#[test]
fn the_nist_test_data_set_gives_the_published_deviations() {
    let curve = Curve::measure(&sp_1065(), 1, |_, _| false);
    for (tau, want) in NIST_SP_1065 {
        let point = curve
            .points
            .iter()
            .find(|p| (p.tau - tau).abs() < 1e-9)
            .unwrap_or_else(|| panic!("no point at tau {tau}"));
        let error = (point.deviation - want).abs() / want;
        assert!(
            error < PUBLISHED_TOLERANCE,
            "tau {tau}: {} against {want}, relative error {error}",
            point.deviation
        );
    }
}
