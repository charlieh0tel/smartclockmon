// Chart helpers shared by the pages that draw time series: what each
// column is called and in what units, and how a y axis is fitted to
// it.  Loaded after common.js.
"use strict";

// Name, the factor that puts the column in units worth reading, and
// an optional qualifier said beside the name in the chart's title
// only: the menu and the cursor readout use the bare name.
// The interval is stored in seconds and is a few nanoseconds: plotted
// as seconds, every axis tick rounds to 0.
const LABELS = {
  efc_percent: ["EFC, percent of range", 1],
  efc_dac: ["EFC, raw counts", 1],
  temperature_c: ["Internal temperature, C", 1],
  oven_current: ["Oven current", 1],
  oven_tempco: ["Oven current coefficient, EFC counts per unit", 1],
  time_interval_s: ["1 PPS TI, ns", 1e9],
  tfom: ["Time figure of merit", 1, "lower is better"],
  ffom: ["Frequency figure of merit", 1, "lower is better"],
  // The receiver's own words, said as counts because that is what is
  // plotted.  Nothing is added to them: "not tracked" is the count
  // `:GPS:SATellite:VISible:PREDicted?` gives -- "the list of
  // satellites (PRN) that the almanac predicts should be visible,
  // given date, time, and position" (097-59551-02 5-6) -- less those
  // being tracked.  Whether one is unusable or merely unneeded is not
  // something the receiver says.
  tracking: ["# sats tracked", 1],
  not_tracking: ["# sats not tracked", 1],
};
const columnName = (c) => (LABELS[c] ?? [c, 1])[0];
const label = (c) => {
  const qualifier = LABELS[c]?.[2];
  return qualifier ? `${columnName(c)} (${qualifier})` : columnName(c);
};
const factor = (c) => (LABELS[c] ?? [c, 1])[1];
const scaled = (values, c) =>
  factor(c) === 1 ? values : values.map((v) => (v === null ? null : v * factor(c)));

// Points to ask for: enough that a long window is not thinned to
// half-hour bins, bounded by what the server will give.
function pointsFor(seconds) {
  if (!seconds) return 5000;
  return Math.min(5000, Math.max(1500, Math.round(seconds / 30)));
}

// A y scale that begins and ends on a labelled gridline.
//
// uPlot scales to the data and then places round ticks inside it, so
// the top and bottom lines fall short by whatever the data happens to
// need -- by a different amount on every plot, which across a stack
// reads as ragged: a label at the top of one, none at the bottom of the
// next.  Snapping the scale itself to the tick step fixes it, at the
// cost of some empty space above and below the trace.
//
// Steps are 1, 2, 5 or 10 times a power of ten, so the labels are
// numbers a person would choose.  2.5 is excluded deliberately: it is a
// legitimate step and it produces axes like 35.925, which is not.
const TICK_COUNT_WEIGHT = 0.3;

// Columns that count things.  A count has no fractional value and
// cannot be negative, so its axis starts at zero and steps in whole
// numbers; the general rule would give a satellite count that never
// moved off zero an axis from -1 to 1, with a tick at -0.5.
// Each is pinned to the top of its range rather than fitted to the
// data, so the axis does not rescale as the window moves and two
// receivers can be read against one another.  TFOM runs 1 to 9 and
// FFOM 0 to 3 by definition; the satellite counts are bounded by the
// channels, and grow if a receiver ever exceeds the pin.
const COUNTS = { tracking: 8, not_tracking: 12, tfom: 9, ffom: 3 };

function countScale(max, pinned, target) {
  const top = Math.max(pinned, Math.ceil(max) || 0, 1);
  const most = Math.max(4, target + 3);
  // A step that divides the top exactly, so the axis ends on the pin
  // rather than above it: TFOM runs to 9, and rounding up to a step of
  // 2 gave it an axis to 10 and a grid line at a figure of merit the
  // receiver cannot report.
  for (const step of [1, 2, 3, 4, 5, 6, 10, 12, 20, 25, 50]) {
    if (top % step) continue;
    const n = top / step;
    if (n >= 2 && n <= most) return { lo: 0, hi: top, step, n };
  }
  for (const step of [1, 2, 5, 10, 20, 50]) {
    const hi = Math.ceil(top / step) * step;
    const n = hi / step;
    if (n >= 2 && n <= most) return { lo: 0, hi, step, n };
  }
  return { lo: 0, hi: top, step: top, n: 1 };
}

function niceScale(min, max, target) {
  if (!isFinite(min) || !isFinite(max)) return null;
  // A series that never moved still needs an axis with room around it.
  //
  // "Never moved" cannot be exact equality.  A constant column still
  // arrives with a spread: SQLite averages each bucket, and summing
  // the same number repeatedly in floating point does not give it back
  // exactly.  Oven current held at 104.9 came through spanning 2e-14,
  // which is not flat by `===`, so the step came out at 1e-15 and the
  // axis read 104.900000000000020.  The oven current coefficient,
  // which is constant between calibrations, would do the same.
  const noise = Math.max(Math.abs(min), Math.abs(max)) * 1e-9;
  if (max - min <= noise) {
    const pad = Math.max(Math.abs(min) * 0.05, 1);
    min -= pad;
    max += pad;
  }
  const span = max - min;
  const mag = Math.pow(10, Math.floor(Math.log10(span / target)));
  let best = null;
  for (const m of [1, 2, 5, 10]) {
    const step = m * mag;
    const lo = Math.floor(min / step) * step;
    const hi = Math.ceil(max / step) * step;
    const n = Math.round((hi - lo) / step);
    // Two lines is not an axis; nine is a grid.
    if (n < 2 || n > 8) continue;
    // Both matter and the balance is empirical.  Weighting the tick
    // count heavily gave internal temperature an axis of 30 to 45 for
    // data spanning 33.8 to 40.3 -- round, correctly bounded, and
    // using less than half the plot.  At this weight the same data
    // gets 32 to 42, which is still round and fills two thirds.
    const score = Math.abs(n - target) * TICK_COUNT_WEIGHT + (hi - lo - span) / span;
    if (!best || score < best.score) best = { lo, hi, step, n, score };
  }
  return best;
}

// How many gaps between labels suit a plot of this height.  About
// forty pixels each: closer and the labels crowd, further and a short
// plot ends up with two.
function tickTarget(height) {
  return Math.min(6, Math.max(3, Math.round(height / 40)));
}

// How tall each chart is, given how many there are.  One gets the room
// it always had; a stack divides a similar budget, with a floor below
// which a plot stops being readable.
function plotHeight(count) {
  return count === 1 ? 420 : Math.max(150, Math.round(560 / count));
}

// One tick label, at the precision its step implies.  Fixed places
// rather than toLocaleString's default, or a step of 0.05 renders as
// 35.9 and 36 and the axis looks unevenly spaced when it is not.
// Not named `tick`, which common.js uses for the status strip's poller.
function tickLabel(v, places) {
  return v.toLocaleString(undefined, {
    minimumFractionDigits: places,
    maximumFractionDigits: places,
  });
}

// A y axis for `values` of column `col` on a plot `height` pixels
// tall: the range its scale is snapped to, and the axis's ticks,
// labels and width to match, so the lines and the range cannot
// disagree.  Both empty when there is nothing to fit.
function yAxis(col, values, height) {
  const nice = !values.length
    ? null
    : col in COUNTS
      ? countScale(Math.max(...values), COUNTS[col], tickTarget(height))
      : niceScale(Math.min(...values), Math.max(...values), tickTarget(height));
  const places = nice ? Math.max(0, -Math.floor(Math.log10(nice.step))) : 0;
  const ticks = [];
  if (nice) {
    for (let i = 0; i <= nice.n; i++) ticks.push(nice.lo + i * nice.step);
  }
  // Wide enough for the longest label this axis will actually draw.
  // Measured from the ticks rather than from the data, since a tick
  // can be wider than every value under it.  The default gutter
  // silently clips instead of overflowing, which turned an EFC of
  // 712,793 into an axis reading 12,793 -- a plausible number, wrong
  // by seven hundred thousand.
  const widest = Math.max(
    ...(ticks.length ? ticks : [0]).map((v) => tickLabel(v, places).length),
    4,
  );
  return {
    axis: {
      size: 30 + widest * 8,
      ...(nice
        ? {
            splits: () => ticks,
            values: (u, vals) => vals.map((v) => tickLabel(v, places)),
          }
        : {}),
    },
    range: nice ? [nice.lo, nice.hi] : null,
  };
}

// ---------------------------------------------------------------- notes

// How near the cursor must be to a note's line, in CSS pixels, for its
// text to be shown.
const NOTE_REACH = 6;

// Each note's line, dashed across the plot area at its time, in its
// own color.  `marks` is a list of `{ at, text, colour }`, `at` in
// unix seconds.
function drawNoteLines(u, marks) {
  const [lo, hi] = [u.scales.x.min, u.scales.x.max];
  const { ctx } = u;
  ctx.save();
  ctx.lineWidth = devicePixelRatio;
  ctx.setLineDash([4 * devicePixelRatio, 4 * devicePixelRatio]);
  for (const { at, colour } of marks) {
    if (at < lo || at > hi) continue;
    const x = Math.round(u.valToPos(at, "x", true));
    ctx.strokeStyle = colour;
    ctx.beginPath();
    ctx.moveTo(x, u.bbox.top);
    ctx.lineTo(x, u.bbox.top + u.bbox.height);
    ctx.stroke();
  }
  ctx.restore();
}

// The text of the note in `marks` under the cursor, if one is within
// reach.
function noteAt(u, marks) {
  if (u.cursor.left == null || u.cursor.left < 0) return null;
  const near = marks.find(({ at }) => Math.abs(u.valToPos(at, "x") - u.cursor.left) <= NOTE_REACH);
  return near ? near.text : null;
}

// A note's text beside the pointer on `chart`, in the page's
// `#note-tip`; hidden when there is none.
function noteTip(chart, text) {
  const tip = $("note-tip");
  if (!chart || text == null) {
    tip.style.display = "none";
    return;
  }
  const box = chart.over.getBoundingClientRect();
  tip.textContent = text;
  tip.style.left = `${box.left + chart.cursor.left + 12}px`;
  tip.style.top = `${box.top + chart.cursor.top + 12}px`;
  tip.style.display = "block";
}

// ----------------------------------------------------------- log axes

// Both axes are decades, which is the only way this curve is read: the
// slope between decades is what names the noise.
const LOG = { distr: 3, log: 10 };

// An axis ends on a named graduation -- 1, 2 or 5 in some decade --
// at or beyond the data, so the ends of the axis are labelled and a
// curve that starts at 10.0004 s (the receiver's ten-second cadence
// is not exactly ten) still gets its graduation at 10.  uPlot's own
// log range rounds out to whole decades, which for a curve ending
// just past 10⁻⁹ leaves most of a decade empty above it.
// A value within a percent of a graduation counts as on it: the
// last tau of a run is 1000.04 s, not 1000, and an axis to 2000 for
// that would be most of a decade of nothing.
const STEPS = [1, 2, 5, 10];
const SNAP = 0.01;
function snapDown(v) {
  const e = Math.floor(Math.log10(v) + 1e-9);
  const m = v / 10 ** e;
  return [...STEPS].reverse().find((s) => s <= m * (1 + SNAP)) * 10 ** e;
}
function snapUp(v) {
  const e = Math.floor(Math.log10(v) - 1e-9);
  const m = v / 10 ** e;
  return STEPS.find((s) => s >= m * (1 - SNAP)) * 10 ** e;
}
const TAU_SCALE = { ...LOG, range: (_, min, max) => [snapDown(min), snapUp(max)] };

// uPlot's axis text defaults to 12px, which is small for numbers that
// are read off, not just glanced at.
const AXIS_FONT = "14px system-ui, sans-serif";

// A value as a mantissa and a power, for the readout and the table,
// where the exact figure matters and a bare decade would not do.
function decade(v) {
  if (!Number.isFinite(v) || v <= 0) return "--";
  const exponent = Math.floor(Math.log10(v));
  const mantissa = v / 10 ** exponent;
  return `${mantissa.toFixed(mantissa < 10 ? 1 : 0)}×${tenTo(exponent)}`;
}

// A log ruler: a graduation at every 1 to 9 of each decade, so the
// spacing between them shows the scale is logarithmic and a point can
// be read off between labels.  uPlot's own splits label 1, 2 and 5 of
// each decade, which is what made the axis look busy -- the answer is
// fewer labels, not fewer graduations, so only the decades are named.
//
// A span too narrow to hold two decades keeps uPlot's own splits,
// rather than being given an axis with one mark on it.
function graduations(u, axis, min, max) {
  const first = Math.floor(Math.log10(min) - 1e-9);
  const last = Math.ceil(Math.log10(max) + 1e-9);
  const out = [];
  for (let e = first; e <= last; e++) {
    for (let m = 1; m < 10; m++) {
      const v = m * 10 ** e;
      if (v >= min * (1 - 1e-9) && v <= max * (1 + 1e-9)) out.push(v);
    }
  }
  return out.length > 1 ? out : null;
}

// The mantissa of a graduation, 1 to 9, as an integer.  Found from the
// rounded exponent rather than by dividing, because 1e-9 divided out
// does not land exactly on 1.
function mantissa(v) {
  const e = Math.floor(Math.log10(v) + 1e-9);
  return Math.round(v / 10 ** e);
}

// Both axes name 1, 2 and 5 in each decade: a curve spanning two
// decades has only three decade lines, and the reader is left
// counting grid lines to place 50 s or 2 × 10⁻¹⁰.  An axis spanning
// less than that has too few of those, and names every graduation.
function labels(splits, format) {
  const sparse = splits.filter((v) => [1, 2, 5].includes(mantissa(v)));
  const name = sparse.length >= 3 ? (v) => sparse.includes(v) : () => true;
  return splits.map((v) => (name(v) ? format(v) : null));
}

// A decade of sigma_y, which is dimensionless and always small.
// Written 10 to the power rather than as 1e-9, which is a programming
// language's spelling of a number rather than a physicist's.
const SUPERSCRIPT = { "-": "⁻", 0: "⁰", 1: "¹", 2: "²",
                      3: "³", 4: "⁴", 5: "⁵", 6: "⁶",
                      7: "⁷", 8: "⁸", 9: "⁹" };
// Ten to the power `exponent`, written with superscripts.
const tenTo = (exponent) => "10" + [...String(exponent)].map((c) => SUPERSCRIPT[c]).join("");

// A graduation as mantissa and power, the mantissa written even when it
// is 1, so 1×10⁻⁹ reads in line with 2×10⁻⁹ and 5×10⁻⁹ beside it.
function power(v) {
  if (!Number.isFinite(v) || v <= 0) return "";
  return `${mantissa(v)}×${tenTo(Math.floor(Math.log10(v) + 1e-9))}`;
}

// A graduation as the number itself: 1, 2, 5, 10, 20, 50.  Past six figures
// the exponent is shorter than the number and just as readable.
function plain(v) {
  if (!Number.isFinite(v) || v <= 0) return "";
  const exponent = Math.floor(Math.log10(v) + 1e-9);
  const m = mantissa(v);
  return exponent < 6 ? String(m * 10 ** exponent) : `${m}e${exponent}`;
}

function seconds(v) {
  if (!Number.isFinite(v)) return "--";
  if (v >= 86400) return `${(v / 86400).toFixed(v < 864000 ? 1 : 0)} d`;
  if (v >= 3600) return `${(v / 3600).toFixed(1)} h`;
  if (v >= 60) return `${(v / 60).toFixed(1)} m`;
  return `${v.toFixed(v < 10 ? 2 : 0)} s`;
}

// The tau axis, the same on both charts.
function tauAxis() {
  return {
    scale: "x",
    stroke: "#8b929c",
    grid: { stroke: "#2b3038" },
    label: "averaging time τ, seconds",
    labelSize: 28,
    font: AXIS_FONT,
    labelFont: AXIS_FONT,
    splits: graduations,
    // uPlot's own filter for a log axis keeps only the decades; which
    // graduations are named is decided in `values` instead.
    filter: (_, splits) => splits,
    values: (_, splits) => labels(splits, plain),
  };
}

function sigmaAxis(label) {
  return {
    scale: "y",
    stroke: "#8b929c",
    grid: { stroke: "#2b3038" },
    size: 72,
    label,
    labelSize: 28,
    font: AXIS_FONT,
    labelFont: AXIS_FONT,
    splits: graduations,
    filter: (_, splits) => splits,
    values: (_, splits) => labels(splits, power),
  };
}

// A y scale that ranges over the bands as well as the lines, so a wide
// interval at long tau is not cut off at the axis.  uPlot ranges from
// the series alone; the shading is not one.
function sigmaScale(bands) {
  const values = Object.values(bands).flat(2).filter((v) => v !== null && v > 0);
  const lo = Math.min(...values), hi = Math.max(...values);
  return {
    ...LOG,
    range: (_, min, max) => [snapDown(Math.min(min, lo)), snapUp(Math.max(max, hi))],
  };
}
