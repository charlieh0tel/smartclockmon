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

/// `(tau, modified deviation, time deviation, averages)`: allantools
/// 2024.06's `mdev` and `tdev` on [`record`], from the same script.
const ALLANTOOLS_MODIFIED: [(f64, f64, f64, usize); 8] = [
    (1.0, 2.9224743293789544e-10, 1.6872913407667106e-10, 998),
    (2.0, 1.5827977093502853e-10, 1.8276573671322206e-10, 995),
    (5.0, 9.721337880183817e-11, 2.8063085210037164e-10, 986),
    (10.0, 6.174602727629926e-11, 3.564908546936135e-10, 971),
    (20.0, 3.7782143150033406e-11, 4.3627061036465524e-10, 941),
    (50.0, 2.875598805014855e-11, 8.301138720783463e-10, 851),
    (100.0, 2.1682883491810615e-11, 1.2518618620804151e-09, 701),
    (200.0, 6.993803402371697e-12, 8.075748554037241e-10, 401),
];

/// `(tau, maximum time interval error, windows)`: allantools 2024.06's
/// `mtie` on [`record`], from the same script.  Its window is `m + 1`
/// readings, so a record of N gives N − m windows.
const ALLANTOOLS_MTIE: [(f64, f64, usize); 8] = [
    (1.0, 4.98628240078049e-10, 999),
    (2.0, 9.434592318457829e-10, 998),
    (5.0, 1.886488288820949e-09, 995),
    (10.0, 2.7156027293371057e-09, 990),
    (20.0, 3.7173854353453e-09, 980),
    (50.0, 5.488951159636002e-09, 950),
    (100.0, 6.463082877901885e-09, 900),
    (200.0, 8.043591440442759e-09, 800),
];

/// One row of [`ALLANTOOLS_CONFIDENCE`].
type Confidence = (f64, i32, f64, f64, f64, f64, f64, f64);

/// `(tau, alpha, ADEV edf, lower, upper, MDEV edf, lower, upper)`:
/// allantools 2024.06's `autocorr_noise_id`, `edf_greenhall` and
/// `confidence_interval` on [`record`], from the same script.  Past
/// 20 s the decimated record is under 30 readings and allantools
/// refuses to identify the noise; the script carries the last answer
/// forward there, as SP 1065 section 5.3.2 says to and the estimator
/// does.
const ALLANTOOLS_CONFIDENCE: [Confidence; 8] = [
    (
        1.0,
        0,
        781.2476904305386,
        2.8512623470858836e-10,
        2.99930302729298e-10,
        781.2476904305386,
        2.851262347085883e-10,
        2.99930302729298e-10,
    ),
    (
        2.0,
        0,
        540.1393885617488,
        1.9526420236305432e-10,
        2.0752281896443654e-10,
        478.51575250794025,
        1.534020608626519e-10,
        1.6365444086664258e-10,
    ),
    (
        5.0,
        0,
        252.3167462999645,
        1.2769063522513976e-10,
        1.3959694558486576e-10,
        191.37750825996238,
        9.260304422187647e-11,
        1.0258874605315632e-10,
    ),
    (
        10.0,
        1,
        247.05607572839637,
        8.774414797004607e-11,
        9.601656549717044e-11,
        98.01619557149583,
        5.77706256561351e-11,
        6.667385171784683e-11,
    ),
    (
        20.0,
        0,
        69.37549430068353,
        4.969117735342777e-11,
        5.8935060691368e-11,
        46.10762995665464,
        3.4393229691848016e-11,
        4.242105061571357e-11,
    ),
    (
        50.0,
        0,
        27.771428571428576,
        3.5110974247420115e-11,
        4.60682154813049e-11,
        17.065461104964694,
        2.4880991431272054e-11,
        3.527691283733153e-11,
    ),
    (
        100.0,
        0,
        12.8,
        2.7514623534674184e-11,
        4.1284945363384156e-11,
        7.4069423739850135,
        1.7723568254216228e-11,
        3.052916081673161e-11,
    ),
    (
        200.0,
        0,
        5.395795202485174,
        1.3126653371066367e-11,
        2.5096562073052028e-11,
        2.7427174224573427,
        5.281079742252766e-12,
        1.387334461931952e-11,
    ),
];

/// Agreement expected between two double-precision implementations of
/// the same sum.
const RELATIVE_TOLERANCE: f64 = 1e-9;

/// Agreement expected between the Wilson-Hilferty chi-squared quantile
/// and scipy's exact one, in the deviation, down to the 2.7 degrees of
/// freedom the record's last point has.
const BOUNDS_TOLERANCE: f64 = 5e-3;

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

#[test]
fn a_gapless_record_agrees_with_allantools_on_the_modified_deviation() {
    let curve = Curve::measure(&record(), 1, |_, _| false);
    for (tau, want, want_time, want_averages) in ALLANTOOLS_MODIFIED {
        let point = curve
            .points
            .iter()
            .find(|p| (p.tau - tau).abs() < 1e-9)
            .unwrap_or_else(|| panic!("no point at tau {tau}"));
        let modified = point
            .modified
            .unwrap_or_else(|| panic!("no modified deviation at tau {tau}"));
        assert_eq!(modified.averages, want_averages, "averages at tau {tau}");
        let error = (modified.deviation - want).abs() / want;
        assert!(
            error < RELATIVE_TOLERANCE,
            "tau {tau}: {} against {want}, relative error {error}",
            modified.deviation
        );
        let error = (modified.time - want_time).abs() / want_time;
        assert!(
            error < RELATIVE_TOLERANCE,
            "tau {tau}: time {} against {want_time}, relative error {error}",
            modified.time
        );
    }
}

#[test]
fn a_gapless_record_agrees_with_allantools_on_the_maximum_excursion() {
    let curve = Curve::measure(&record(), 1, |_, _| false);
    for (tau, want, want_windows) in ALLANTOOLS_MTIE {
        let point = curve
            .points
            .iter()
            .find(|p| (p.tau - tau).abs() < 1e-9)
            .unwrap_or_else(|| panic!("no point at tau {tau}"));
        let mtie = point
            .mtie
            .unwrap_or_else(|| panic!("no excursion at tau {tau}"));
        assert_eq!(mtie.windows, want_windows, "windows at tau {tau}");
        // A maximum of differences of the same doubles: exact.
        assert!(
            (mtie.peak - want).abs() <= want * 1e-15,
            "tau {tau}: {} against {want}",
            mtie.peak
        );
    }
}

/// `(tau, overlapping Allan deviation, modified Allan deviation, time
/// deviation)` for the 1000-point test data set, from NIST SP 1065
/// (`third_party/NIST-SP-1065.pdf`) Table 31.
const NIST_SP_1065: [(f64, f64, f64, f64); 3] = [
    (1.0, 2.922319e-01, 2.922319e-01, 1.687202e-01),
    (10.0, 9.159953e-02, 6.172376e-02, 3.563623e-01),
    (100.0, 3.241343e-02, 2.170921e-02, 1.253382e-00),
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
    for (tau, want, want_modified, want_time) in NIST_SP_1065 {
        let point = curve
            .points
            .iter()
            .find(|p| (p.tau - tau).abs() < 1e-9)
            .unwrap_or_else(|| panic!("no point at tau {tau}"));
        let modified = point
            .modified
            .unwrap_or_else(|| panic!("no modified deviation at tau {tau}"));
        for (name, got, want) in [
            ("overlapping Allan", point.deviation, want),
            ("modified Allan", modified.deviation, want_modified),
            ("time", modified.time, want_time),
        ] {
            let error = (got - want).abs() / want;
            assert!(
                error < PUBLISHED_TOLERANCE,
                "tau {tau}, {name} deviation: {got} against {want}, relative error {error}"
            );
        }
    }
}

#[test]
fn a_gapless_record_agrees_with_allantools_on_the_confidence_intervals() {
    let curve = Curve::measure(&record(), 1, |_, _| false);
    for (tau, alpha, edf, lower, upper, edf_modified, lower_modified, upper_modified) in
        ALLANTOOLS_CONFIDENCE
    {
        let point = curve
            .points
            .iter()
            .find(|p| (p.tau - tau).abs() < 1e-9)
            .unwrap_or_else(|| panic!("no point at tau {tau}"));
        assert_eq!(point.noise, Some(alpha), "noise type at tau {tau}");
        let bounds = point
            .bounds
            .unwrap_or_else(|| panic!("no interval at tau {tau}"));
        let modified = point
            .modified
            .and_then(|m| m.bounds)
            .unwrap_or_else(|| panic!("no modified interval at tau {tau}"));
        for (name, got, want, tolerance) in [
            ("edf", bounds.edf, edf, RELATIVE_TOLERANCE),
            ("lower", bounds.lower, lower, BOUNDS_TOLERANCE),
            ("upper", bounds.upper, upper, BOUNDS_TOLERANCE),
            (
                "modified edf",
                modified.edf,
                edf_modified,
                RELATIVE_TOLERANCE,
            ),
            (
                "modified lower",
                modified.lower,
                lower_modified,
                BOUNDS_TOLERANCE,
            ),
            (
                "modified upper",
                modified.upper,
                upper_modified,
                BOUNDS_TOLERANCE,
            ),
        ] {
            let error = (got - want).abs() / want;
            assert!(
                error < tolerance,
                "tau {tau} {name}: {got} against {want}, relative error {error}"
            );
        }
    }
}
