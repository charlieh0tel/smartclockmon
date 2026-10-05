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
//
// Not `tick`: that name was already taken by the status-strip poller
// above, and a second declaration of it silently replaced the first --
// so boot called this with no arguments, threw on undefined, and left
// the strip reading "connecting..." for ever while the charts drew
// fine.
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
