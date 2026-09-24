//! The overlapping Allan deviation against an independent
//! implementation.
//!
//! The values below are allantools 2024.06's `oadev` on the same phase
//! record; `adev_reference.py` beside this file regenerates them.  The
//! estimator's own tests check what it does with gaps, holds and
//! segments, which allantools has no opinion on; this checks that on a
//! plain gapless record the number it produces is the standard one.

use jiff::SignedDuration;
use jiff::Timestamp;
use smartclock::adev::Curve;
use smartclock::adev::Sample;

/// `(tau, deviation, differences)` from `adev_reference.py`.
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
