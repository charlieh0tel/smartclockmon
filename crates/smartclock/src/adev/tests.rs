//! The estimator on made-up records: gaps, holds, segments, and the
//! identities the three statistics must satisfy.
use super::Curve;
use super::MAX_SAMPLES;
use super::Run;
use super::Sample;
use super::spacing;
use super::stride;
use super::updates;
use jiff::Timestamp;

/// A run of `n` readings one second apart, from `f`.
fn run(n: usize, f: impl Fn(usize) -> f64) -> Vec<Sample> {
    let start = Timestamp::from_second(1_700_000_000).expect("a timestamp");
    (0..n)
        .map(|i| Sample {
            at: start + jiff::SignedDuration::from_secs(i as i64),
            interval: f(i),
        })
        .collect()
}

fn gapless(samples: &[Sample]) -> Run {
    Run::from_samples(samples, 1.0, |_, _| false)
}

#[test]
fn a_long_run_does_not_drift_off_its_own_grid() {
    // Twenty-four hours of one-second readings with half a second
    // of jitter on each.  A tau0 taken from a single median gap is
    // wrong by parts per million, which accumulates past half a
    // grid step before the day is out and starts dropping readings
    // near the end.  Every reading must land.
    let jitter = |i: usize| {
        let x = ((i as f64) * 7.13).sin() * 1237.71;
        ((x - x.floor()) - 0.5) * 0.4
    };
    let start = Timestamp::from_second(1_700_000_000).expect("a timestamp");
    let count = 86_400;
    let samples: Vec<Sample> = (0..count)
        .map(|i| Sample {
            at: start
                + jiff::SignedDuration::from_nanos(((i as f64 + jitter(i)) * 1e9).round() as i64),
            interval: 1e-9 * i as f64,
        })
        .collect();
    let tau0 = spacing(&samples).expect("a spacing");
    assert!((tau0 - 1.0).abs() < 1e-7, "tau0 {tau0}");
    let run = Run::from_samples(&samples, tau0, |_, _| false);
    let (present, holes) = run.coverage();
    assert_eq!(present, count, "readings were dropped");
    assert_eq!(holes, 0);
}

#[test]
fn the_averaging_times_are_one_two_five_by_decade() {
    let day = Run::from_samples(&run(86_400, |i| 1e-9 * i as f64), 1.0, |_, _| false);
    let taus: Vec<f64> = day.curve().iter().map(|p| p.tau).collect();
    assert_eq!(
        taus,
        vec![
            1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0,
            20000.0,
        ]
    );
}

#[test]
fn a_holdover_the_interval_sat_still_through_still_breaks_the_run() {
    // Locked, then five seconds of holdover during which the
    // interval holds one value, then locked again with the phase
    // stepped by 5 us.  Every holdover row is a repeat and is
    // dropped, but the run must still break there, or the step is
    // read as instability.  Five seconds keeps the gap between the
    // kept readings under the ten intervals at which an absence
    // splits the run by itself, which would hide the case.
    #[derive(PartialEq)]
    enum State {
        Locked,
        Holdover,
    }
    let samples = run(400, |i| match i {
        0..200 => 1e-9 * (i % 7) as f64,
        200..205 => 1e-9 * (199 % 7) as f64,
        _ => 5e-6 + 1e-9 * (i % 7) as f64,
    });
    let states: Vec<State> = (0..400)
        .map(|i| {
            if (200..205).contains(&i) {
                State::Holdover
            } else {
                State::Locked
            }
        })
        .collect();
    let curve = Curve::from_readings(&samples, &states);
    assert!(curve.segments >= 2, "{} segments", curve.segments);
    for point in &curve.points {
        // The 5 us step would put tau = 1 near 3e-6.
        assert!(point.deviation < 1e-8, "{point:?}");
    }
}

#[test]
fn a_restart_that_moves_the_polling_phase_does_not_bend_the_spacing() {
    // Two runs of readings exactly a second apart, the second
    // starting a thousand and a half seconds after the first ended:
    // a daemon restarted polls on a new phase.  No whole number of
    // steps spans the gap, and fitting one line through both runs
    // bends it.
    let start = Timestamp::from_second(1_700_000_000).expect("a timestamp");
    let at = |millis: i64| start + jiff::SignedDuration::from_millis(millis);
    let samples: Vec<Sample> = (0..200)
        .map(|i| at(i * 1000))
        .chain((0..200).map(|i| at(199_000 + 1_000_500 + i * 1000)))
        .map(|at| Sample { at, interval: 0.0 })
        .collect();
    let tau0 = spacing(&samples).expect("a spacing");
    assert!((tau0 - 1.0).abs() < 1e-9, "tau0 {tau0}");
}

#[test]
fn a_gap_that_splits_the_run_splits_it_at_every_stride() {
    // Fifteen seconds missing from one-second readings, with the
    // phase stepped across them: more than ten readings' spacing,
    // less than ten of a stride-two grid's.
    let samples: Vec<Sample> = run(400, |i| if i < 200 { 0.0 } else { 5e-6 })
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !(200..215).contains(i))
        .map(|(_, sample)| sample)
        .collect();
    for stride in [1, 2] {
        let curve = Curve::measure(&samples, stride, |_, _| false);
        assert_eq!(curve.segments, 2, "stride {stride}");
    }
}

#[test]
fn a_holdover_with_no_interval_still_breaks_the_run() {
    // Locked, then five seconds of holdover in which the receiver
    // refused the interval, then locked with the phase stepped by
    // 5 us.  The holdover rows carry no reading, but they carry the
    // state, and the run must break there.
    #[derive(PartialEq)]
    enum State {
        Locked,
        Holdover,
    }
    let start = Timestamp::from_second(1_700_000_000).expect("a timestamp");
    let rows = (0..400i64).map(|i| {
        let at = start + jiff::SignedDuration::from_secs(i);
        let jitter = 1e-9 * (i % 7) as f64;
        match i {
            0..200 => (at, Some(jitter), State::Locked),
            200..205 => (at, None, State::Holdover),
            _ => (at, Some(5e-6 + jitter), State::Locked),
        }
    });
    let curve = Curve::from_logged(rows);
    assert!(curve.segments >= 2, "{} segments", curve.segments);
    for point in &curve.points {
        assert!(point.deviation < 1e-8, "{point:?}");
    }
}

#[test]
fn readings_are_strided_only_past_the_limit() {
    assert_eq!(stride(0), 1);
    assert_eq!(stride(MAX_SAMPLES), 1);
    assert_eq!(stride(MAX_SAMPLES + 1), 2);
    assert_eq!(stride(2 * MAX_SAMPLES), 2);
    assert_eq!(stride(2 * MAX_SAMPLES + 1), 3);
}

#[test]
fn a_held_reading_counts_once() {
    // The receiver updates its interval every ten seconds and
    // answers with the held value in between.  Ten identical
    // readings are one measurement, and counting them as ten puts
    // second differences of zero into the short taus.
    let held = run(100, |i| 1e-9 * (i / 10) as f64);
    let keep = updates(&held);
    assert_eq!(keep, (0..10).map(|i| i * 10).collect::<Vec<_>>());
    let once: Vec<Sample> = keep.iter().map(|&i| held[i]).collect();
    // And the spacing found is the receiver's, not the poll rate.
    let tau0 = spacing(&once).expect("a spacing");
    assert!((tau0 - 10.0).abs() < 1e-6, "tau0 {tau0}");
}

#[test]
fn a_coarse_grid_keeps_the_reading_nearest_each_point() {
    // Gridding one-second readings at ten seconds must keep the
    // ones at 0, 10, 20 and not the ones at 5, 15, 25.  Keeping
    // whichever came first does the latter, which dates a phase
    // five seconds early and shows up as instability that is not
    // there.  A steady ramp proves it: any misplacement breaks it.
    let samples = run(600, |i| 4e-9 * i as f64);
    let coarse = Run::from_samples(&samples, 10.0, |_, _| false);
    for point in coarse.curve() {
        assert!(point.deviation < 1e-15, "{point:?}");
    }
    let (present, holes) = coarse.coverage();
    assert_eq!(present, 60, "one reading per grid point");
    assert_eq!(holes, 0);
}

#[test]
fn a_long_absence_does_not_desynchronise_the_caller() {
    // The absence rule can split without asking, so a caller that
    // counted its own calls would be one behind for the rest of the
    // run and declare its breaks in the wrong places.  Here the
    // caller is asked about every adjacent pair and about no other,
    // whatever the absence rule does.
    let start = Timestamp::from_second(1_700_000_000).expect("a timestamp");
    let mut samples = run(20, |_| 0.0);
    samples.extend((0..20).map(|i| Sample {
        at: start + jiff::SignedDuration::from_secs(3600 + i as i64),
        interval: 0.0,
    }));
    let mut asked = Vec::new();
    Run::from_samples(&samples, 1.0, |a, b| {
        asked.push((a, b));
        false
    });
    assert_eq!(asked.len(), samples.len() - 1);
    assert!(
        asked
            .iter()
            .enumerate()
            .all(|(i, pair)| *pair == (i, i + 1))
    );
}

#[test]
fn a_perfect_clock_has_no_deviation() {
    // A constant offset and a constant rate are both removed by the
    // second difference, which is the point of using one: a clock
    // that is wrong but steady is not an unstable clock.
    let curve = gapless(&run(600, |i| 1e-6 + 2e-9 * i as f64)).curve();
    assert!(!curve.is_empty());
    for point in &curve {
        // Not zero: a second difference of doubles near 1e-6
        // leaves rounding residue around 1e-22.  The bound is far
        // below anything a real oscillator could show.
        assert!(point.deviation < 1e-15, "{point:?}");
    }
}

#[test]
fn white_phase_noise_falls_as_one_over_tau() {
    // Independent phase errors of the same size at every sample:
    // the textbook case, where sigma_y is proportional to 1/tau.
    // Deterministic pseudo-noise rather than a generator, so the
    // test cannot fail on a bad seed.
    let noise = |i: usize| {
        let x = ((i as f64) * 12.9898).sin() * 43758.5453;
        (x - x.floor()) - 0.5
    };
    let curve = gapless(&run(4000, |i| 1e-8 * noise(i))).curve();
    let first = curve.first().expect("a point at tau = 1");
    let decade = curve
        .iter()
        .find(|p| (p.tau - 10.0).abs() < 1e-9)
        .expect("a point at tau = 10");
    // A decade of tau should cost a decade of deviation.  The
    // tolerance is wide because this is an estimate, not an
    // identity.
    let ratio = first.deviation / decade.deviation;
    assert!((3.0..30.0).contains(&ratio), "ratio {ratio}");
}

#[test]
fn at_one_reading_the_modified_deviation_is_the_plain_one() {
    // With m = 1 each window mean is one reading, so the two sums
    // are the same sum over the same triples.
    let noise = |i: usize| {
        let x = ((i as f64) * 12.9898).sin() * 43758.5453;
        (x - x.floor()) - 0.5
    };
    let point = gapless(&run(500, |i| 1e-8 * noise(i)))
        .at(1)
        .expect("a point at tau = 1");
    let modified = point.modified.expect("a modified deviation");
    assert_eq!(modified.averages, point.differences);
    let error = (modified.deviation - point.deviation).abs() / point.deviation;
    assert!(error < 1e-12, "{point:?}");
    let time = point.tau * modified.deviation / 3f64.sqrt();
    assert!((modified.time - time).abs() < 1e-30, "{modified:?}");
}

#[test]
fn white_phase_noise_falls_faster_on_the_modified_deviation() {
    // The modified form averages white phase noise down inside
    // each window, so it falls as tau^-3/2 where the plain form
    // falls as 1/tau: a decade of tau costs about a decade and a
    // half.
    let noise = |i: usize| {
        let x = ((i as f64) * 12.9898).sin() * 43758.5453;
        (x - x.floor()) - 0.5
    };
    let curve = gapless(&run(4000, |i| 1e-8 * noise(i))).curve();
    let at = |tau: f64| {
        curve
            .iter()
            .find(|p| (p.tau - tau).abs() < 1e-9)
            .and_then(|p| p.modified)
            .unwrap_or_else(|| panic!("no modified point at tau {tau}"))
    };
    let ratio = at(1.0).deviation / at(10.0).deviation;
    assert!((10.0..100.0).contains(&ratio), "ratio {ratio}");
}

#[test]
fn the_modified_deviation_of_window_means_is_that_of_the_readings() {
    // The receiver's reading is the mean of ten one-second
    // readings.  Averaging a one-second record into contiguous
    // ten-second means and measuring at the same tau must give the
    // same modified deviation, since the estimator's own windows
    // are made of whole means; the estimates differ only in how
    // many overlapping windows each record offers.
    let noise = |i: usize| {
        let x = ((i as f64) * 12.9898).sin() * 43758.5453;
        (x - x.floor()) - 0.5
    };
    let fine = run(6000, |i| 1e-8 * noise(i));
    let start = Timestamp::from_second(1_700_000_000).expect("a timestamp");
    let means: Vec<Sample> = fine
        .chunks(10)
        .enumerate()
        .map(|(k, block)| Sample {
            at: start + jiff::SignedDuration::from_secs(10 * k as i64),
            interval: block.iter().map(|s| s.interval).sum::<f64>() / 10.0,
        })
        .collect();
    let of_fine = gapless(&fine)
        .at(100)
        .and_then(|p| p.modified)
        .expect("fine");
    let of_means = Run::from_samples(&means, 10.0, |_, _| false)
        .at(10)
        .and_then(|p| p.modified)
        .expect("means");
    let ratio = of_means.deviation / of_fine.deviation;
    assert!((0.9..1.1).contains(&ratio), "ratio {ratio}");
    assert_eq!(of_fine.averages, 6000 - 300 + 1);
    assert_eq!(of_means.averages, 600 - 30 + 1);
}

#[test]
fn a_steady_ramp_has_an_excursion_of_exactly_its_slope_times_tau() {
    // A clock 3 ns/s fast wanders by 3 ns per second of window,
    // whichever window: the peak-to-peak over m + 1 readings is
    // m times the step, and every window of the run is counted.
    let ramp = gapless(&run(600, |i| 3e-9 * i as f64));
    for m in [1, 10, 100] {
        let mtie = ramp.at(m).and_then(|p| p.mtie).expect("an excursion");
        assert!((mtie.peak - 3e-9 * m as f64).abs() < 1e-18, "{mtie:?}");
        assert_eq!(mtie.windows, 600 - m);
    }
}

#[test]
fn a_hole_ends_every_window_that_would_span_it() {
    // One reading missing from six hundred: at m = 10 the eleven
    // windows that would include it are not examined, and a
    // spike buried in the missing reading cannot be seen.
    let mut samples = run(600, |i| 3e-9 * i as f64);
    samples.remove(200);
    let mtie = gapless(&samples)
        .at(10)
        .and_then(|p| p.mtie)
        .expect("an excursion");
    assert_eq!(mtie.windows, 590 - 11);
    assert!((mtie.peak - 3e-8).abs() < 1e-18, "{mtie:?}");
}

#[test]
fn a_single_spike_sets_the_excursion_at_every_tau_that_sees_it() {
    // The deviations average a lone 1 us spike away; the maximum
    // time interval error is the spike, at every tau.
    let spiked = gapless(&run(600, |i| if i == 300 { 1e-6 } else { 0.0 }));
    for point in spiked.curve() {
        let mtie = point.mtie.expect("an excursion");
        assert!((mtie.peak - 1e-6).abs() < 1e-18, "{point:?}");
    }
}

#[test]
fn a_hole_costs_the_modified_deviation_every_window_that_spans_it() {
    // One reading missing from six hundred.  The plain form loses
    // the three triples that use it; the modified form at m = 10
    // loses the thirty whose three windows cover it.
    let mut samples = run(600, |i| 3e-9 * i as f64);
    samples.remove(200);
    let point = gapless(&samples).at(10).expect("a point at tau = 10");
    assert_eq!(point.differences, 580 - 3);
    let modified = point.modified.expect("a modified deviation");
    assert_eq!(modified.averages, 571 - 30);
    assert!(modified.deviation < 1e-15, "{modified:?}");
}

#[test]
fn a_hole_is_skipped_rather_than_closed_up() {
    // Dropping a sample must not slide the ones after it one place
    // earlier: that would turn a gap into a phase step and read as
    // instability.  The proof is that a steady ramp with a hole in
    // it still has no deviation.
    let mut samples = run(600, |i| 3e-9 * i as f64);
    samples.remove(200);
    samples.remove(201);
    let curve = gapless(&samples).curve();
    assert!(!curve.is_empty());
    for point in &curve {
        assert!(point.deviation < 1e-15, "{point:?}");
    }
    let (present, holes) = gapless(&samples).coverage();
    assert_eq!(present, 598);
    assert_eq!(holes, 2);
}

#[test]
fn a_long_absence_splits_the_run() {
    let start = Timestamp::from_second(1_700_000_000).expect("a timestamp");
    let mut samples = run(300, |i| 1e-9 * i as f64);
    // An hour later, and 50 us away: the receiver was off and came
    // back somewhere else.
    samples.extend((0..300).map(|i| Sample {
        at: start + jiff::SignedDuration::from_secs(3600 + i as i64),
        interval: 5e-5 + 1e-9 * i as f64,
    }));
    let both = gapless(&samples);
    assert_eq!(both.segments(), 2);
    // The step between them is 50 us.  Were it counted, tau = 1
    // would show a deviation near 1e-5; it must not appear at all.
    for point in both.curve() {
        assert!(point.deviation < 1e-15, "{point:?}");
    }
}

#[test]
fn a_declared_discontinuity_splits_the_run() {
    let samples = run(600, |i| if i < 300 { 0.0 } else { 1e-4 });
    let split = Run::from_samples(&samples, 1.0, |previous, sample| {
        (samples[sample].interval - samples[previous].interval).abs() > 1e-6
    });
    assert_eq!(split.segments(), 2);
    for point in split.curve() {
        assert!(point.deviation < 1e-15, "{point:?}");
    }
    // Without the split the same step is read as instability.
    let whole = gapless(&samples).curve();
    assert!(whole[0].deviation > 1e-6, "{:?}", whole[0]);
}

#[test]
fn the_curve_stops_where_the_data_does() {
    // Sixty samples reach tau = 20, a third of the run, and stop
    // there whatever the caller asks for.
    let short = gapless(&run(60, |i| 1e-9 * i as f64));
    let taus: Vec<f64> = short.curve().iter().map(|p| p.tau).collect();
    assert_eq!(taus, vec![1.0, 2.0, 5.0, 10.0, 20.0]);
    assert!(short.at(100).is_none());
}

#[test]
fn the_nearest_of_two_readings_in_a_slot_is_kept() {
    // A reading 300 ms from its slot, a nanosecond off the ramp,
    // lands in the same slot as the reading made on time.  Keeping
    // it rather than the nearer one puts a nanosecond step into a
    // ramp that is otherwise perfectly steady.
    let start = Timestamp::from_second(1_700_000_000).expect("a timestamp");
    let mut samples = run(600, |i| 1e-9 * i as f64);
    samples.push(Sample {
        at: start + jiff::SignedDuration::from_millis(300),
        interval: 1e-9,
    });
    samples.sort_by_key(|s| s.at);
    let run = gapless(&samples);
    let (present, holes) = run.coverage();
    assert_eq!((present, holes), (600, 0));
    let points = run.curve();
    assert!(!points.is_empty());
    for point in &points {
        assert!(
            point.deviation < 1e-15,
            "the farther reading was kept: {point:?}"
        );
    }
}
