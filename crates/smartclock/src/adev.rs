//! Overlapping Allan deviation from the receiver's 1 PPS time interval.
//!
//! `:SYNChronization:TINTerval?` is a phase reading, so a run of them
//! is a phase series `x(t)`, which is what the Allan deviation is
//! defined over.
//!
//! What it is the phase *of* is worth being exact about.  The interval
//! measured is between the 1 PPS from the GPS receiver and a 1 PPS
//! derived from the OCXO -- "the time difference between the 1-pps
//! signal from the GPS engine to a similar signal derived from the
//! reference source", in the words of the design paper
//! (`smartclock-dec96a9`, Enhanced Learning).
//!
//! The GPS receiver's 1 PPS comes from its own crystal and is
//! quantized to it.  While locked, the OCXO is steered to follow that
//! 1 PPS.  So the curve is of the pair and of the loop between them,
//! and not of the OCXO alone.
//!
//! The estimator is the overlapping one, which uses every available
//! triple rather than every third sample and so has far more degrees of
//! freedom at the same run length:
//!
//! ```text
//!                    1
//! sigma_y(tau)^2 = ------------  sum ( x[i+2m] - 2 x[i+m] + x[i] )^2
//!                  2 N tau^2
//! ```
//!
//! with `tau = m * tau0` and `N` the number of differences that existed.
//!
//! Gaps are handled by counting only the differences that exist rather than
//! by filling anything in.  That leaves the estimate unbiased so long as
//! what is missing is unrelated to what was being measured -- readings
//! lost to a daemon restart are; readings lost *because* the receiver
//! was misbehaving would not be, and no estimator can rescue that.

use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::Deserialize;
use serde::Serialize;

/// One phase reading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// When it was read.
    pub at: Timestamp,
    /// The 1 PPS to GPS interval, in seconds.
    pub interval: f64,
}

/// One point of the deviation curve.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    /// Averaging time, in seconds.
    pub tau: f64,
    /// Overlapping Allan deviation at that tau, dimensionless.
    pub deviation: f64,
    /// How many second differences went into it.
    ///
    /// Each is `x[i+2m] - 2 x[i+m] + x[i]`, so each needs three phase
    /// readings present at the right spacing, which is what a gap
    /// takes away.  Reported rather than hidden because it is the whole
    /// confidence story: a point from nine differences and one from
    /// nine thousand are drawn the same size and mean very different
    /// things.
    pub differences: usize,
}

/// The fewest second differences a point may be computed from.
///
/// Below this the estimate is dominated by its own uncertainty -- the
/// relative error of an overlapping estimate goes roughly as
/// `1/sqrt(differences)` -- and plotting it invites reading a slope off
/// noise.  A short run therefore ends early rather than trailing off
/// into points nobody should believe.
const MIN_DIFFERENCES: usize = 10;

/// How much of a gap in the timestamps ends a segment.
///
/// Expressed as a multiple of the nominal spacing.  One missing sample
/// is a hole to be skipped over, not a discontinuity: the estimator
/// simply finds no triple there.  A long absence is different, because
/// the daemon was not watching and the receiver may have done anything
/// in between.
const GAP_SEGMENTS_AFTER: f64 = 10.0;

/// A run of phase readings with nothing discontinuous inside it.
///
/// Segments exist because a second difference taken across a break is
/// not a measurement of anything.  Between them the phase may have
/// jumped -- the receiver stepping its own clock, a relock after
/// holdover, a different receiver entirely -- and the estimator would
/// read that jump as enormous instability at every tau it spans.
#[derive(Debug, Clone, Default)]
pub struct Segment {
    /// The readings, in time order.
    pub samples: Vec<Sample>,
}

/// Phase readings divided into segments, on a common grid.
#[derive(Debug, Clone)]
pub struct Run {
    /// Nominal spacing between readings, in seconds.
    tau0: f64,
    /// Each segment as a regular grid, `None` where a reading is
    /// missing.  Gridding is what makes gaps cost nothing: an index is
    /// a time, so a triple is checked for existence rather than
    /// assumed from position in a list.
    grids: Vec<Vec<Option<f64>>>,
}

impl Run {
    /// Grid a set of segments at `tau0` seconds.
    ///
    /// Readings are placed at `round((t - t0) / tau0)`, so jitter in
    /// the poll schedule does not accumulate into a drift between a
    /// reading's index and its time.  Two readings landing on one index
    /// keep the first: at that point they are the same measurement seen
    /// twice, which is what a repeated row in the log is.
    pub fn new(segments: &[Segment], tau0: f64) -> Self {
        let grids = segments
            .iter()
            .filter(|s| !s.samples.is_empty())
            .map(|segment| Self::grid(&segment.samples, tau0))
            .collect();
        Self { tau0, grids }
    }

    /// Place one segment's readings on a regular grid.
    ///
    /// When two readings fall on the same grid point the nearer one
    /// wins, and a grid point with no reading close enough to it stays
    /// empty.
    ///
    /// Both rules matter once the grid is coarser than the readings.
    /// Gridding a one-second run at ten seconds, keeping whichever
    /// reading arrived first would hold the one from t=5 at the grid
    /// point for t=10, and the estimator would treat a phase measured
    /// at t=5 as though it had been measured five seconds later.  And
    /// the last grid point of a run always has *some* nearest reading,
    /// up to half a grid step away; filling it from that one invents a
    /// measurement.  So the tolerance is half a reading interval, not
    /// half a grid step: a grid point is filled only when something was
    /// actually measured at about that time.
    fn grid(samples: &[Sample], tau0: f64) -> Vec<Option<f64>> {
        let start = samples[0].at;
        // In grid units, so it can be compared with the rounding error
        // directly.  Never more than half a grid step, which is all
        // that rounding can produce anyway.
        let tolerance = (0.5 * spacing(samples).unwrap_or(tau0) / tau0).min(0.5);
        let mut grid: BTreeMap<usize, (f64, f64)> = BTreeMap::new();
        for sample in samples {
            let offset = (sample.at - start).total(jiff::Unit::Second).unwrap_or(0.0);
            let exact = offset / tau0;
            let index = exact.round();
            if index < 0.0 || (exact - index).abs() > tolerance {
                continue;
            }
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "checked non-negative, and bounded by the segment rule"
            )]
            let index = index as usize;
            let error = (exact - index as f64).abs();
            match grid.get(&index) {
                Some((best, _)) if *best <= error => {}
                _ => {
                    grid.insert(index, (error, sample.interval));
                }
            }
        }
        let last = grid.keys().next_back().copied().unwrap_or(0);
        (0..=last)
            .map(|i| grid.get(&i).map(|(_, value)| *value))
            .collect()
    }

    /// Divide readings into segments and grid them.
    ///
    /// `discontinuous` is given the indices of two adjacent readings
    /// and says whether the receiver did something between them that
    /// makes the phase either side incomparable.  Indices rather than
    /// the readings themselves so the answer depends on nothing but
    /// its arguments: a closure counting its own calls would be
    /// desynchronised by the long-absence rule below, which can split
    /// without consulting it.
    ///
    /// A long absence splits a segment on its own, since the daemon was
    /// not watching and cannot say what happened.
    pub fn from_samples(
        samples: &[Sample],
        tau0: f64,
        mut discontinuous: impl FnMut(usize, usize) -> bool,
    ) -> Self {
        let mut segments: Vec<Segment> = Vec::new();
        let mut current = Segment::default();
        for (i, sample) in samples.iter().enumerate() {
            if let Some(previous) = current.samples.last() {
                let apart = (sample.at - previous.at)
                    .total(jiff::Unit::Second)
                    .unwrap_or(0.0);
                // Both tested, and neither short-circuited: the caller
                // may be recording what it is asked about.
                let absent = apart > tau0 * GAP_SEGMENTS_AFTER;
                let declared = discontinuous(i - 1, i);
                if absent || declared {
                    segments.push(std::mem::take(&mut current));
                }
            }
            current.samples.push(*sample);
        }
        segments.push(current);
        Self::new(&segments, tau0)
    }

    /// The deviation curve, one point per averaging time the data
    /// supports.
    ///
    /// Taus are spaced roughly logarithmically, because that is how the
    /// curve is read: a linear sweep spends every point at the noisy
    /// end and never reaches the interesting one.  The sweep stops at
    /// the first tau with too few differences behind it rather than running
    /// to some fraction of the run length, so the curve ends where the
    /// data does.
    pub fn curve(&self) -> Vec<Point> {
        let longest = self.grids.iter().map(Vec::len).max().unwrap_or(0);
        let mut points = Vec::new();
        for m in multipliers(longest, self.tau0) {
            match self.at(m) {
                Some(point) => points.push(point),
                None => break,
            }
        }
        points
    }

    /// The deviation at one averaging time, or `None` if too little of
    /// the run supports it.
    ///
    /// Segments are pooled: each contributes the differences it has, and
    /// the sum and the count run across all of them.  Pooling is what
    /// makes a broken run usable at all -- averaging each segment's own
    /// deviation would weight a two-minute fragment like a two-day run.
    pub fn at(&self, m: usize) -> Option<Point> {
        if m == 0 {
            return None;
        }
        let mut total = 0.0;
        let mut differences = 0usize;
        for grid in &self.grids {
            if grid.len() <= 2 * m {
                continue;
            }
            for i in 0..grid.len() - 2 * m {
                let (Some(a), Some(b), Some(c)) = (grid[i], grid[i + m], grid[i + 2 * m]) else {
                    continue;
                };
                let difference = c - 2.0 * b + a;
                total += difference * difference;
                differences += 1;
            }
        }
        if differences < MIN_DIFFERENCES {
            return None;
        }
        let tau = m as f64 * self.tau0;
        Some(Point {
            tau,
            deviation: (total / (2.0 * differences as f64 * tau * tau)).sqrt(),
            differences,
        })
    }

    /// How many readings are on the grid, and how many places are
    /// holes.
    ///
    /// A curve says nothing about how complete the run behind it was,
    /// and a run that is mostly holes can still produce a confident
    /// looking line.
    pub fn coverage(&self) -> (usize, usize) {
        let present = self
            .grids
            .iter()
            .flatten()
            .filter(|slot| slot.is_some())
            .count();
        let places = self.grids.iter().map(Vec::len).sum::<usize>();
        (present, places.saturating_sub(present))
    }

    /// How many segments the run broke into.
    pub fn segments(&self) -> usize {
        self.grids.len()
    }
}

/// How many phase readings one curve is computed from at full rate.
///
/// A day at one reading a second is 86400, so this covers better than
/// two days before a caller has to grid more coarsely.  It bounds both
/// the memory a grid takes and the work a sweep does, neither of which
/// a caller asking for the whole log should be able to set.
pub const MAX_SAMPLES: usize = 250_000;

/// A deviation curve and what it was computed from.
///
/// The counts travel with the curve because a curve alone cannot be
/// judged: one drawn from a run that is mostly holes, or that broke
/// into forty segments, looks exactly like one drawn from a clean day.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Curve {
    /// Nominal spacing between readings, in seconds.
    pub tau0: f64,
    /// How many unbroken runs the range divided into.
    pub segments: usize,
    /// Readings used.
    pub present: usize,
    /// Places on the grid where a reading was expected and missing.
    pub holes: usize,
    /// The curve itself.
    pub points: Vec<Point>,
}

impl Curve {
    /// Measure readings as logged: one per poll, each with the state the
    /// receiver was in beside it.
    ///
    /// Held repeats are dropped (see [`updates`]), and a run is broken
    /// between two kept readings if the state changed anywhere between
    /// them -- among the dropped rows as well as the kept ones.  Checking
    /// only the kept pair missed a holdover the interval sat still
    /// through: every row of it was a repeat, all were dropped, the
    /// readings either side were both locked, and the phase step at the
    /// relock was measured as instability.
    ///
    /// A range too long to grid at full rate is gridded more coarsely;
    /// see [`MAX_SAMPLES`].
    pub fn from_readings<S: PartialEq>(samples: &[Sample], states: &[S]) -> Self {
        let keep = updates(samples);
        let kept: Vec<Sample> = keep.iter().map(|&i| samples[i]).collect();
        // broken[k]: the state changed somewhere after kept reading k-1
        // and up to kept reading k.
        let broken: Vec<bool> = keep
            .iter()
            .enumerate()
            .map(|(k, &i)| {
                k > 0 && (keep[k - 1] + 1..=i).any(|j| states.get(j) != states.get(j - 1))
            })
            .collect();
        let stride = kept.len().div_euclid(MAX_SAMPLES) + 1;
        Self::measure(&kept, stride, |_, b| broken[b])
    }

    /// Measure a set of readings, splitting them where `discontinuous`
    /// says the phase either side is incomparable.
    ///
    /// `stride` grids more coarsely than the readings arrive, which is
    /// a subsample rather than an average: averaging readings before
    /// the estimator sees them is the operation it exists to perform.
    /// It costs the short taus, which is the right trade for a long
    /// range and the wrong one for a short range, so the caller picks.
    pub fn measure(
        samples: &[Sample],
        stride: usize,
        discontinuous: impl FnMut(usize, usize) -> bool,
    ) -> Self {
        let tau0 = spacing(samples).unwrap_or(1.0) * stride.max(1) as f64;
        let run = Run::from_samples(samples, tau0, discontinuous);
        let (present, holes) = run.coverage();
        Self {
            tau0,
            segments: run.segments(),
            present,
            holes,
            points: run.curve(),
        }
    }
}

/// One sample per reading the receiver actually made.
///
/// `:SYNChronization:TINTerval?` is answered from a value the receiver
/// updates on its own schedule, not when it is asked: measured on a
/// 58503A, 397 of 400 distinct values were held for exactly ten
/// one-second polls.  Polling faster than that returns the same number
/// again, and a repeat is not a measurement.
///
/// Left in, the repeats destroy the short end of the curve rather than
/// merely padding it: the second difference of a held value is zero,
/// so every triple inside one update reads as perfect stability and
/// `sigma_y` at the shortest taus is dragged towards nothing.
///
/// Two consecutive updates that happen to land on the same value are
/// merged with them, which loses one sample and leaves a hole the
/// estimator already knows how to skip.  At the resolution these are
/// reported to -- 0.1 ns against a typical step of some nanoseconds --
/// that is rare enough to prefer over keeping the repeats.
/// Returns the indices to keep, so a caller holding anything alongside
/// the readings -- the mode and holdover flag that decide where a run
/// is cut -- can drop the same ones and stay in step.
pub fn updates(samples: &[Sample]) -> Vec<usize> {
    let mut keep: Vec<usize> = Vec::with_capacity(samples.len());
    let mut held: Option<f64> = None;
    for (i, sample) in samples.iter().enumerate() {
        if held != Some(sample.interval) {
            held = Some(sample.interval);
            keep.push(i);
        }
    }
    keep
}

/// The interval between readings, in seconds, taken from the readings
/// themselves rather than from the configured cadence.
///
/// Two passes.  The median of the gaps is robust -- neither a long
/// absence nor a run of late polls moves it -- but it is only ever one
/// observed gap, so it carries that gap's jitter: a nominal one second
/// measured over a few hundred real polls came out as 0.999992636.
///
/// That error cannot be left in.  The grid places a reading at
/// `round((t - t0) / tau0)`, so a relative error in `tau0` accumulates
/// against the run: seven parts per million reaches half a grid step
/// after about seventy thousand readings, and from there readings miss
/// their slots and are dropped -- worse, periodically, as the drift
/// wraps.  A day at one reading a second is within sight of that.
///
/// So the median only assigns each reading a whole-number position,
/// and the spacing is then the least-squares slope of time against
/// position.  Dividing the total span by the total number of steps
/// would also average out the jitter in between, but its error is set
/// by the jitter on the two end readings alone and stays around
/// `jitter / n`; a slope over every reading falls off as
/// `n^-3/2`, which is the difference between four tenths of a grid
/// step of drift across a day and four thousandths.
pub fn spacing(samples: &[Sample]) -> Option<f64> {
    let offsets: Vec<f64> = samples
        .iter()
        .filter_map(|s| (s.at - samples.first()?.at).total(jiff::Unit::Second).ok())
        .collect();
    let mut gaps: Vec<f64> = offsets.windows(2).map(|w| w[1] - w[0]).collect();
    gaps.retain(|gap| *gap > 0.0);
    if gaps.is_empty() {
        return None;
    }
    gaps.sort_by(f64::total_cmp);
    let median = gaps[gaps.len() / 2];

    // Whole-number positions, stepped by the rounded gap.  An absence
    // steps by however many readings it swallowed, which is what keeps
    // the positions a straight line through the whole run rather than
    // one line per segment.
    let mut position = 0.0;
    let positions: Vec<f64> = offsets
        .windows(2)
        .map(|w| {
            position += ((w[1] - w[0]) / median).round().max(0.0);
            position
        })
        .collect();
    let count = positions.len() as f64 + 1.0;
    let mean_position = (0.0 + positions.iter().sum::<f64>()) / count;
    let mean_offset = offsets.iter().sum::<f64>() / count;
    let pairs = std::iter::once((0.0, offsets[0])).chain(
        positions
            .iter()
            .copied()
            .zip(offsets.iter().copied().skip(1)),
    );
    let (covariance, variance) = pairs.fold((0.0, 0.0), |(covariance, variance), (p, t)| {
        let dp = p - mean_position;
        (covariance + dp * (t - mean_offset), variance + dp * dp)
    });
    if variance <= 0.0 {
        return Some(median);
    }
    Some(covariance / variance)
}

/// Averaging times to offer, in seconds: 1, 2 and 5 in every decade,
/// the sequence a log axis is labelled with, so the points sit on the
/// graduations.
fn ladder() -> impl Iterator<Item = f64> {
    (0..8).flat_map(|decade| {
        [1.0, 2.0, 5.0]
            .into_iter()
            .map(move |step| step * 10f64.powi(decade))
    })
}

/// The ladder as multiples of the sample spacing, for a run of
/// `longest` grid places.
///
/// A rung the spacing cannot express -- anything under half a sample
/// -- is dropped, and two rungs that round to the same multiple are
/// kept once.  The reported tau is still the multiple times the
/// spacing, never the rung: the rung chooses the measurement, it does
/// not describe it.
fn multipliers(longest: usize, tau0: f64) -> Vec<usize> {
    // Past a third of the run an overlapping estimate has too few
    // independent differences to mean much whatever the count says, so
    // the sweep never proposes one.
    let ceiling = longest / 3;
    let mut out: Vec<usize> = Vec::new();
    for tau in ladder() {
        if tau0 <= 0.0 {
            break;
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the ladder is small and positive, and m is checked below"
        )]
        let m = (tau / tau0).round() as usize;
        if m == 0 {
            continue;
        }
        if m > ceiling {
            break;
        }
        if out.last() != Some(&m) {
            out.push(m);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::Curve;
    use super::Run;
    use super::Sample;
    use super::spacing;
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
                    + jiff::SignedDuration::from_nanos(
                        ((i as f64 + jitter(i)) * 1e9).round() as i64
                    ),
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
                1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0,
                10000.0, 20000.0,
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
    fn a_repeated_reading_is_one_measurement() {
        // The log holds a row per publish, and the medium and slow
        // steps publish the fast tier's last reading again.  Two rows
        // at the same instant are one sample, not two.
        let start = Timestamp::from_second(1_700_000_000).expect("a timestamp");
        let mut samples = run(600, |i| 1e-9 * i as f64);
        samples.push(Sample {
            at: start + jiff::SignedDuration::from_millis(300),
            interval: 1e-9,
        });
        samples.sort_by_key(|s| s.at);
        let (present, holes) = gapless(&samples).coverage();
        assert_eq!(present, 600);
        assert_eq!(holes, 0);
    }
}
