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
//! Beside it, the modified Allan deviation (NIST SP 1065 section 5.2.5,
//! equation 14) takes the same second difference of phase *averaged*
//! over `m` consecutive readings, `A[j] = mean(x[j..j+m])`:
//!
//! ```text
//!                        1
//! Mod sigma_y(tau)^2 = ------------  sum ( A[j+2m] - 2 A[j+m] + A[j] )^2
//!                      2 N tau^2
//! ```
//!
//! and the time deviation is `tau * Mod sigma_y(tau) / sqrt(3)` (equation
//! 15): the phase wander over `tau`, in seconds, whose log-log slope
//! is the modified deviation's plus one -- falling only while phase
//! noise dominates, rising once frequency noise does.  The modified
//! form is the one that fits this record exactly.
//! The receiver's reading is already the mean of ten one-second
//! readings over a contiguous window (`docs/firmware.md`, "The
//! ten-second average"), so the mean of `m` consecutive readings is the
//! mean of `10 m` consecutive one-second readings: the averaging the
//! receiver did is the innermost block of the averaging the estimator
//! does, and Mod sigma_y from the record equals Mod sigma_y of the
//! underlying one-second phase at every tau of one reading and above.
//! The plain Allan deviation has no such identity: the receiver's
//! averaging divides the white phase noise's variance by ten, and for
//! white PM the Allan variance is `3 sigma_x^2 / tau^2` at every tau,
//! so the plain deviation of the record sits a factor `sqrt(10)` below
//! that of the 1 PPS wherever that noise dominates -- on a bench
//! 58503A, everywhere out to 500 s (`PLAN.md`).
//!
//! The maximum time interval error (SP 1065 section 5.2.9) is the third
//! figure: over every window of `m + 1` consecutive readings, the
//! largest peak-to-peak excursion of the phase, in seconds.  It is the
//! worst case, not an average, so one relock or sawtooth step sets it
//! for every tau that spans the step; that is what a timing mask is
//! written against, and why it is reported beside the deviations
//! rather than inferred from them.
//!
//! Gaps are handled by counting only the differences that exist rather than
//! by filling anything in.  That leaves the estimate unbiased so long as
//! what is missing is unrelated to what was being measured -- readings
//! lost to a daemon restart are; readings lost *because* the receiver
//! was misbehaving would not be, and no estimator can rescue that.  The
//! modified form needs every reading of its three windows, so one hole
//! costs it `3 m` triples where the plain form loses three.

use std::collections::BTreeMap;
use std::collections::VecDeque;

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
    /// The modified Allan deviation at the same tau, when enough
    /// averaged triples exist for it.  Fewer do than for the plain
    /// form, since each needs `3 m` consecutive readings, so a curve
    /// can carry the plain deviation a rung or two past the modified.
    pub modified: Option<Modified>,
    /// The maximum time interval error at the same tau, when at least
    /// one window of `m + 1` consecutive readings exists.
    pub mtie: Option<Excursion>,
}

/// The maximum time interval error at one averaging time.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Excursion {
    /// The largest peak-to-peak phase excursion within any window of
    /// `tau`, in seconds.
    pub peak: f64,
    /// How many windows were examined, each needing `m + 1` consecutive
    /// readings present.  A worst case over ten windows and one over
    /// ten thousand mean different things, as with the deviations.
    pub windows: usize,
}

/// The modified Allan deviation at one averaging time, with the time
/// deviation that follows from it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Modified {
    /// Modified Allan deviation, dimensionless.
    pub deviation: f64,
    /// Time deviation, `tau * deviation / sqrt(3)`, in seconds.
    pub time: f64,
    /// How many second differences of averaged phase went into it,
    /// each needing `3 m` consecutive readings present.
    pub averages: usize,
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
/// Expressed as a multiple of the spacing the readings arrived at, not
/// of the grid: a grid coarsened for a long range would otherwise let
/// the same absence through that splits a short one.  One missing sample
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
    /// not watching and cannot say what happened.  Long is judged
    /// against the readings' own spacing, whatever `tau0` is.
    pub fn from_samples(
        samples: &[Sample],
        tau0: f64,
        mut discontinuous: impl FnMut(usize, usize) -> bool,
    ) -> Self {
        let absence = spacing(samples).unwrap_or(tau0) * GAP_SEGMENTS_AFTER;
        let mut segments: Vec<Segment> = Vec::new();
        let mut current = Segment::default();
        for (i, sample) in samples.iter().enumerate() {
            if let Some(previous) = current.samples.last() {
                let apart = (sample.at - previous.at)
                    .total(jiff::Unit::Second)
                    .unwrap_or(0.0);
                // Both tested, and neither short-circuited: the caller
                // may be recording what it is asked about.
                let absent = apart > absence;
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
        let (total, differences) = self.pooled(|grid| Self::plain(grid, m));
        let (averaged, averages) = self.pooled(|grid| Self::modified(grid, m));
        let (peak, windows) = self
            .grids
            .iter()
            .fold((0.0f64, 0), |(peak, windows), grid| {
                let (p, w) = Self::excursions(grid, m);
                (peak.max(p), windows + w)
            });
        if differences < MIN_DIFFERENCES {
            return None;
        }
        let tau = m as f64 * self.tau0;
        let deviation = |sum: f64, count: usize| (sum / (2.0 * count as f64 * tau * tau)).sqrt();
        let modified = (averages >= MIN_DIFFERENCES).then(|| {
            let deviation = deviation(averaged, averages);
            Modified {
                deviation,
                time: tau * deviation / 3f64.sqrt(),
                averages,
            }
        });
        let mtie = (windows > 0).then_some(Excursion { peak, windows });
        Some(Point {
            tau,
            deviation: deviation(total, differences),
            differences,
            modified,
            mtie,
        })
    }

    /// A sum of squares and a count, pooled over every segment.
    fn pooled(&self, f: impl Fn(&[Option<f64>]) -> (f64, usize)) -> (f64, usize) {
        self.grids.iter().fold((0.0, 0), |(sum, count), grid| {
            let (s, c) = f(grid);
            (sum + s, count + c)
        })
    }

    /// The plain form's second differences over one grid: the sum of
    /// their squares and how many existed.
    fn plain(grid: &[Option<f64>], m: usize) -> (f64, usize) {
        if grid.len() <= 2 * m {
            return (0.0, 0);
        }
        (0..grid.len() - 2 * m).fold((0.0, 0), |(sum, count), i| {
            match (grid[i], grid[i + m], grid[i + 2 * m]) {
                (Some(a), Some(b), Some(c)) => {
                    let difference = c - 2.0 * b + a;
                    (sum + difference * difference, count + 1)
                }
                _ => (sum, count),
            }
        })
    }

    /// The modified form's second differences of window means over one
    /// grid: the sum of their squares and how many existed.
    ///
    /// Window means come from running sums, with a running count of
    /// readings so a window with a hole in it is known to be short and
    /// is skipped rather than averaged over fewer.
    fn modified(grid: &[Option<f64>], m: usize) -> (f64, usize) {
        if grid.len() < 3 * m {
            return (0.0, 0);
        }
        let mut sums = Vec::with_capacity(grid.len() + 1);
        let mut counts = Vec::with_capacity(grid.len() + 1);
        sums.push(0.0);
        counts.push(0usize);
        for slot in grid {
            sums.push(sums.last().unwrap_or(&0.0) + slot.unwrap_or(0.0));
            counts.push(counts.last().unwrap_or(&0) + usize::from(slot.is_some()));
        }
        let mean =
            |j: usize| (counts[j + m] - counts[j] == m).then(|| (sums[j + m] - sums[j]) / m as f64);
        (0..=grid.len() - 3 * m).fold((0.0, 0), |(sum, count), j| {
            match (mean(j), mean(j + m), mean(j + 2 * m)) {
                (Some(a), Some(b), Some(c)) => {
                    let difference = c - 2.0 * b + a;
                    (sum + difference * difference, count + 1)
                }
                _ => (sum, count),
            }
        })
    }

    /// The largest excursion over one grid's windows of `m + 1`
    /// consecutive readings, and how many windows there were.
    ///
    /// A sliding maximum and minimum over monotone deques, so the pass
    /// is linear in the grid whatever `m` is.  A hole ends the run of
    /// consecutive readings: the deques are emptied and the count
    /// restarts, so no window spans it.
    fn excursions(grid: &[Option<f64>], m: usize) -> (f64, usize) {
        let mut peak = 0.0f64;
        let mut windows = 0usize;
        // Indices into `grid`, values decreasing (for the maximum) or
        // increasing (for the minimum) from front to back.
        let mut highs: VecDeque<usize> = VecDeque::new();
        let mut lows: VecDeque<usize> = VecDeque::new();
        // How many consecutive readings end at the current index.
        let mut run = 0usize;
        for (i, slot) in grid.iter().enumerate() {
            let Some(x) = *slot else {
                highs.clear();
                lows.clear();
                run = 0;
                continue;
            };
            run += 1;
            while highs
                .back()
                .is_some_and(|&j| grid[j].is_some_and(|y| y <= x))
            {
                highs.pop_back();
            }
            highs.push_back(i);
            while lows
                .back()
                .is_some_and(|&j| grid[j].is_some_and(|y| y >= x))
            {
                lows.pop_back();
            }
            lows.push_back(i);
            if run < m + 1 {
                continue;
            }
            let start = i - m;
            while highs.front().is_some_and(|&j| j < start) {
                highs.pop_front();
            }
            while lows.front().is_some_and(|&j| j < start) {
                lows.pop_front();
            }
            if let (Some(&hi), Some(&lo)) = (highs.front(), lows.front())
                && let (Some(high), Some(low)) = (grid[hi], grid[lo])
            {
                peak = peak.max(high - low);
                windows += 1;
            }
        }
        (peak, windows)
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
        Self::measure(&kept, stride(kept.len()), |_, b| broken[b])
    }

    /// As [`Curve::from_readings`], for rows as the log holds them,
    /// with the interval possibly missing.
    ///
    /// A row without an interval is still evidence of the receiver's
    /// state: a holdover during which the interval was refused lies
    /// entirely in such rows, and leaving them out joined the locked
    /// readings either side across it.  Each stands in as a repeat of
    /// the reading before, which [`updates`] drops as a reading while
    /// its state still counts.  One before any reading has nothing to
    /// repeat and is left out.
    pub fn from_logged<S: PartialEq>(
        rows: impl IntoIterator<Item = (Timestamp, Option<f64>, S)>,
    ) -> Self {
        let mut samples: Vec<Sample> = Vec::new();
        let mut states = Vec::new();
        for (at, interval, state) in rows {
            let Some(interval) = interval.or_else(|| samples.last().map(|s| s.interval)) else {
                continue;
            };
            samples.push(Sample { at, interval });
            states.push(state);
        }
        Self::from_readings(&samples, &states)
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

/// How coarsely to grid `readings`: the least stride that leaves no
/// more than [`MAX_SAMPLES`] of them.
fn stride(readings: usize) -> usize {
    readings.div_ceil(MAX_SAMPLES).max(1)
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
/// position, fitted within each run between absences.  Dividing the total span by the total number of steps
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

    // Whole-number positions within each run, stepped by the rounded
    // gap, and a slope fitted to every run about its own means.  One
    // line through the whole record would need a whole number of steps
    // to span every absence, and a daemon restarted polls on a new
    // phase, so that number does not exist: the fraction left over
    // bends the line.  The same absence that splits the estimator's
    // runs splits these.
    let mut runs: Vec<Vec<(f64, f64)>> = vec![vec![(0.0, offsets[0])]];
    let mut position = 0.0;
    for w in offsets.windows(2) {
        let gap = w[1] - w[0];
        if gap > median * GAP_SEGMENTS_AFTER {
            position = 0.0;
            runs.push(Vec::new());
        } else {
            position += (gap / median).round().max(0.0);
        }
        if let Some(run) = runs.last_mut() {
            run.push((position, w[1]));
        }
    }
    let (covariance, variance) = runs.iter().fold((0.0, 0.0), |totals, run| {
        let count = run.len() as f64;
        let mean_position = run.iter().map(|&(p, _)| p).sum::<f64>() / count;
        let mean_offset = run.iter().map(|&(_, t)| t).sum::<f64>() / count;
        run.iter().fold(totals, |(covariance, variance), &(p, t)| {
            let dp = p - mean_position;
            (covariance + dp * (t - mean_offset), variance + dp * dp)
        })
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
mod tests;
