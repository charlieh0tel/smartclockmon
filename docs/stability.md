# Stability

How the browser's stability page and the monitor compute their curves
from the logged 1 PPS interval, what checks them, and what was
measured.  The decisions are in `PLAN.md`, "Allan deviation is
computed over segments, not over a series"; `docs/views.md` says what
the views show.

`:SYNChronization:TINTerval?` is a phase reading: "the time difference
between the 1-pps signal from the GPS engine to a similar signal
derived from the reference source" (`smartclock-dec96a9`, Enhanced
Learning).  The GPS 1 PPS is quantized to its own crystal and the
locked OCXO is steered to follow it, so the curve is of the pair and
their loop, not of the OCXO alone.

The estimator is the overlapping one.  Gaps are handled by counting
only the second differences that exist, unbiased while what is missing
is unrelated to what was measured (a daemon restart, not a misbehaving
receiver).

- **No second difference across a discontinuity.**  A relock,
  holdover, power cycle or receiver swap may step the phase.  The run
  is cut into segments on the recorded mode and holdover flag, and on
  any absence over ten reading intervals (of the readings, not of a
  coarsened grid, so a gap that cuts a short range cuts a long one).
  Segments are pooled.  State is checked on every logged row, including
  held repeats and rows without an interval, where a short holdover can
  lie wholly.
- **A hole stays a hole.**  Readings go on a time-indexed grid, so a
  missing reading is an empty slot, not a phase step.
- **A grid point with no reading near it stays empty.**  Nearest wins
  within half a *reading* interval, not half a grid step; otherwise the
  last point of a run is filled from a reading up to half a step away.
- **The spacing must beat one observed gap.**  The grid places a
  reading at `round((t - t0) / tau0)`, so an error in `tau0`
  accumulates; a median nominal second of 0.999992636 drops readings
  periodically within a day.  The median only assigns whole-number
  positions; the spacing is the least-squares slope of time against
  position (error falling as `n^-3/2`), fitted per run between absences
  about that run's means, since a restarted daemon polls on a new
  phase: one line through two runs of two hundred readings half a
  second apart was bent by four parts in ten thousand.

`crates/smartclock/tests/adev_reference.rs` checks a gapless record:
the 1000-point data set of NIST SP 1065 section 12.4 gives Table 31's
overlapping Allan deviation at tau 1, 10 and 100 to the seven figures
published, and allantools 2024.06's `oadev` on a thousand-reading
record (regeneration script beside the test) agrees to one part in 10^9
at every tau from 1 to 200 s, with the same count of differences.

The web view's query keeps a row only where interval or state changed,
reads at most the newest 500 000 such rows (about two months), and says
when a range held more.  A curve carries its count of readings, holes
and segments, since a mostly-holes run looks like a clean day.  Points
below ten second differences are not emitted; the sweep stops at a
third of the longest segment.  Long ranges are subsampled onto a
coarser grid, not averaged (averaging is the estimator's job), at the
cost of short taus; the caller picks.

**The reading is already an average.**  `docs/firmware/interval.md` shows
`:SYNChronization:TINTerval?` is the mean of ten one-second readings,
updated every ten seconds.  For white PM the Allan variance is 3σx²/τ²
at every tau, so that averaging lowers ADEV by √10 wherever white PM
dominates.  MDEV over n consecutive 10 s means is the window mean over
10n one-second readings, so MDEV of the record equals MDEV of the
one-second phase at every tau of 10 s and above, with fewer overlapping
estimates.  TDEV is tau times MDEV over root three.  So MDEV and TDEV
are the primary curves, from the same gridded segments (a hole voids
every window spanning it, `3m` triples rather than three); ADEV is
kept, labeled, because data sheets quote it.  Both are checked against
allantools' `mdev` and `tdev` and Table 31's modified Allan and time
deviation columns.

Measured on the bench 58503A, 25 September 2026: one hour of
`:DIAGnostic:PTIMe:TINTerval?` (the one-second reading) beside
`:SYNChronization:TINTerval?`, one pass a second through the daemon.

- The ten-second value is the mean of the ten one-second readings
  ending one poll before it appears, to 0.15 ns rms over 352 holds --
  the reply's 0.1 ns rounding.
- The one-second readings run −72.5 to +45 ns, standard deviation
  30.6 ns, the GPS engine's sawtooth uncorrected.
- allantools on both series:

  | tau, s | ADEV, 1 s readings | ADEV, 10 s means | MDEV, 1 s readings | MDEV, 10 s means |
  | ------ | ------------------ | ---------------- | ------------------ | ---------------- |
  | 10 | 5.34e-9 | 1.68e-9 | 1.69e-9 | 1.68e-9 |
  | 20 | 2.60e-9 | 7.44e-10 | 5.57e-10 | 5.39e-10 |
  | 50 | 1.03e-9 | 3.18e-10 | 1.68e-10 | 1.64e-10 |
  | 100 | 5.25e-10 | 1.67e-10 | 6.89e-11 | 6.72e-11 |
  | 200 | 2.71e-10 | 8.19e-11 | 2.76e-11 | 2.74e-11 |
  | 500 | 1.08e-10 | 3.06e-11 | 6.02e-12 | 6.02e-12 |

  MDEV agrees to 1 to 3 % at every tau, the difference being the count
  of windows.  ADEV from the means is 3.1 to 3.5 times lower at every
  tau to 500 s (√10 is 3.16), because white phase noise dominates the
  whole range: the one-second ADEV falls as 1/τ from 5.2 × 10⁻⁸ at 1 s
  all the way out.  On this receiver the plain Allan deviation of the
  record is not that of the 1 PPS anywhere measured; MDEV is exact
  throughout.

The short end of every curve is the receiver's 1 PPS against the GPS
engine's.  The 58503B specifications (097-58503-12, chapter 4) give
time accuracy as "<110 ns with respect to UTC (USNO MC), 95%
probability" and 1 PPS edge jitter as "<750 ps rms" (the divided-down
OCXO, not the interval to GPS).  The engine's pulse carries an
uncorrected sawtooth the Oncore reports as "-128 .. 127 ns"
(`VPCommands.pdf`, @@Bn/@@En); no path from that report into the
interval was found in the firmware.  A locked bench 58503A read -62 to +81 ns
over six hours, with reading-to-reading jumps up to 94 ns, within the
110 ns; so an MTIE of ~100 ns at short tau meets the specification,
and that jitter is the deviations' short-tau floor.

TDEV has its own chart under the sigma-y chart, sharing tau axis and
cursor, because it is in seconds and its slope is MDEV's plus one
(white PM tau^-1/2, flicker PM flat, white FM tau^1/2, flicker FM tau,
random-walk FM tau^3/2); on a shared plot TDEV rising while MDEV falls
reads as a contradiction.  TDEV says how far the 1 PPS wanders over
tau.

MTIE is on that chart too, computed, not inferred.  For Gaussian noise
it is a few times the rms, but it is set by the largest excursion -- a
sawtooth step, a relock, a holdover hop -- which deviations average
away, so a bound from TDEV can be an order of magnitude short.  It is a sliding
maximum and minimum over `m + 1` consecutive readings (SP 1065 section
5.2.9) through monotone deques, skipping windows spanning a hole or
cut.  Checked against allantools' `mtie`.

Each deviation carries a one-sigma confidence interval, drawn as a band
and tabulated as two asymmetric bounds.  One sigma because stability
plots use it, and a 95 % band on a log axis swallows the curve at long
tau.  The interval is SP 1065 equation 45 with Greenhall's equivalent
degrees of freedom (section 5.4.1, Greenhall and Riley 2003) for the
noise type the lag 1 autocorrelation method identifies at each tau
(sections 5.5.5 and 5.5.6, quadratic detrend as in allantools).  A tau
with too few decimated readings to identify noise takes the previous
tau's (section 5.3.2); a record too short for the edf algorithm (white
PM under three windows) gets no interval, as does MTIE.  Degrees of
freedom pool across segments.  The chi-squared quantile is Wilson and
Hilferty's, within 0.4 % of scipy's from two degrees of freedom up and
2.6 % at one and a half.  Tested against allantools'
`autocorr_noise_id`, `edf_greenhall` and `confidence_interval` on the
reference record, and edf and noise identification on every power law
from white PM to random walk FM, at lengths reaching each branch and
refusal of Greenhall's algorithm.  The TUI draws no band: ratatui has
no fill.

`:DIAGnostic:PTIMe:TINTerval?`, the latest one-second reading, would
extend the curves below 10 s and is not polled: it costs a query a
second and ten times the phase rows, for the receiver-noise floor
alone.  Plain `:PTIMe:TINTerval?` is the same mean as
`:SYNChronization:TINTerval?`, by the image's handler table and by 187
paired polls of the bench 58503A.
