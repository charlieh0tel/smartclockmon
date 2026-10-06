// What every page shares: the status strip, the receiver picker, the
// range control, and the lifecycle that decides when a page reads and
// what it does when the receiver changes or its daemon goes away.
//
// The strip is on every page because the state of the receiver is the
// context for whatever else a page shows: a sky plot or a stability
// curve read without knowing the unit is in holdover is read wrongly.
// The lifecycle is shared so that the pages cannot drift apart in how
// they behave at the edges, which is where they had: one cleared on a
// change of receiver and the others kept the last unit's charts up
// until the new ones arrived, one noticed the daemon going and the
// others did not.  Loaded as a classic script before each page's own,
// so these are ordinary globals, and each page ends by calling
// `content` once.

const $ = (id) => document.getElementById(id);
const pad = (n) => String(n).padStart(2, "0");
// Everything interpolated into innerHTML goes through this.  The values
// are numbers and enums today, but they come from a string scraper over
// a serial line, and one field becoming an Option<String> upstream
// would otherwise make this page execute whatever the receiver said.
const esc = (v) =>
  String(v).replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const fmt = (v, digits = 3) =>
  v === null || v === undefined ? "--" : Number(v).toFixed(digits);
// A state pill: `bad` for an error or a lost link, plain otherwise.
const pill = (text, bad) => `<span class="pill${bad ? " down" : ""}">${esc(text)}</span>`;

// How long the strip and the picker wait for the server.
const QUICK_TIMEOUT = 5000;
// How often the strip polls, and how often the list of receivers is
// read again so a unit that appears or a daemon that comes or goes
// shows in the picker without a reload.
const STRIP_EVERY = 1000;
const RECEIVERS_EVERY = 10000;

// ------------------------------------------------------------- fetching

// Total: never rejects, never hangs.  A rejected fetch anywhere in the
// boot sequence used to abort it before the polling timer was
// installed, leaving the page stuck on "connecting..." until someone
// reloaded it -- the one failure that needed a human.  For the strip
// and the picker, which report failure in place.
async function getJson(url) {
  try {
    const response = await fetch(url, { signal: AbortSignal.timeout(QUICK_TIMEOUT) });
    return await response.json();
  } catch (e) {
    return { error: String(e) };
  }
}

// An answer that arrived for a receiver or a range nobody is looking at
// any more.  The lifecycle drops it without a word.
class Superseded extends Error {}

// Read JSON for a page's content, or throw.  `signal` is the one the
// lifecycle hands the page, aborted when the page moves on; anything
// arriving after that throws Superseded rather than being drawn.  A
// failure the server could describe comes back as 200 with `error` in
// it, and throws with that message.
async function ask(url, signal, timeout = QUICK_TIMEOUT) {
  let body;
  try {
    const r = await fetch(url, {
      signal: AbortSignal.any([signal, AbortSignal.timeout(timeout)]),
    });
    if (!r.ok) throw new Error(`the server answered ${r.status}`);
    body = await r.json();
  } catch (e) {
    if (signal.aborted) throw new Superseded();
    if (e.name === "TimeoutError") throw new Error(`no answer within ${timeout / 1000} s`);
    throw e;
  }
  if (signal.aborted) throw new Superseded();
  if (body?.error) throw new Error(body.error);
  return body;
}

// ------------------------------------------------------------- receiver

// The receiver every page is about.  Null until the list has been read,
// and then always set: the server defaults to the unit seen most
// recently, and pinning that choice keeps a later request from
// disagreeing with an earlier one.
let unit = null;
// The receivers the server last listed.
let receivers = [];

function withUnit(q) {
  if (unit !== null) q.set("receiver", unit);
  return q;
}

// The daemon attached to the chosen receiver, or whatever the address
// names before the list has been read.
function liveQuery() {
  const serial = unit ?? new URLSearchParams(location.search).get("receiver");
  return serial === null ? "" : `?receiver=${encodeURIComponent(serial)}`;
}

// Merge `params` into the address without touching the rest of it: a
// null value removes the key.  Every control that is worth keeping on
// a reload or a shared link -- the receiver, the range, the columns --
// goes through here, so none of them wipes another's setting.  `push`
// makes the change a step the browser's Back button undoes; only a
// change of range is one.
function remember(params, push = false) {
  const q = new URLSearchParams(location.search);
  for (const [k, v] of Object.entries(params)) {
    if (v === null || v === undefined) q.delete(k);
    else q.set(k, String(v));
  }
  const s = q.toString();
  const url = location.pathname + (s ? `?${s}` : "");
  if (push && url !== location.pathname + location.search) history.pushState(null, "", url);
  else history.replaceState(null, "", url);
  remembered = location.search;
}
// The address as this page last wrote it.  Back restores an older
// address whole, and only its range is wanted from it.
let remembered = location.search;

// What the links between pages carry from this one's address: the
// receiver and the range, which mean the same on every page.  The
// columns and the series are one page's own.
const CARRIED = ["receiver", "last", "from", "to"];

// Point the links between pages at the same receiver and range, so
// moving from history to stability keeps both rather than falling back
// to the newest unit and the default hour.
function carry() {
  const here = new URLSearchParams(location.search);
  const q = new URLSearchParams();
  for (const k of CARRIED) {
    if (here.has(k)) q.set(k, here.get(k));
  }
  for (const a of document.querySelectorAll("nav a")) {
    a.search = q.toString();
  }
}

// Keep the chosen receiver in the address, so a reload or a shared
// link does, and on the links between pages.
function carryUnit() {
  remember({ receiver: unit });
  carry();
}

// Whether the chosen receiver measures `column`, by the server's word.
// Taken to until the list has been read, so nothing is hidden early on
// a guess.
function measures(column) {
  const r = receivers.find((x) => x.serial === unit);
  return !r?.columns || r.columns.includes(column);
}

const unitName = (serial) => {
  const r = receivers.find((x) => x.serial === serial);
  return r ? [r.model, r.serial].filter(Boolean).join(" ") : serial ?? "the receiver";
};

// Read the list of receivers and redraw the picker from it.  The unit
// comes from the address when it names one the server lists, and is
// otherwise the first it lists: live ones ahead of history.  A failed
// read keeps the list it had.  The bar is hidden for one unit or none:
// a control whose only option is the one already chosen is noise.
async function listReceivers() {
  const list = await getJson("/api/receivers");
  if (list.error || !Array.isArray(list)) return;
  receivers = list;
  if (unit === null && list.length) {
    const asked = new URLSearchParams(location.search).get("receiver");
    unit = (list.find((r) => r.serial === asked) ?? list[0]).serial;
    carryUnit();
  }
  const bar = $("unit-bar");
  if (list.length < 2) {
    bar.hidden = true;
    return;
  }
  // A unit chosen earlier that the server no longer lists stays chosen,
  // and says so, rather than being swapped for another under the
  // reader's feet.
  const options = list.some((r) => r.serial === unit) ? list : [...list, { serial: unit }];
  // The port too, for a live unit: it is how the bench names them.
  const label = (r) => unitName(r.serial) + (r.instance ? ` on ${r.instance}` : "");
  $("unit").innerHTML = options
    .map((r) => `<option value="${esc(r.serial)}">${esc(label(r))}</option>`)
    .join("");
  $("unit").value = unit;
  const r = list.find((x) => x.serial === unit);
  $("unit-seen").textContent = !r ? "no longer listed"
    : r.instance !== null ? `live on smartclockd@${r.instance}`
    : r.last_seen ? `last seen ${r.last_seen.slice(0, 19)}Z`
    : "";
  bar.hidden = false;
}

// ---------------------------------------------------------------- strip

// Whether the last poll found a daemon with its receiver attached.
// Null until the first poll for the current receiver has answered.
let linkUp = null;
let ticking = false;
let tickAgain = false;

async function tick() {
  if (ticking) {
    tickAgain = true;
    return;
  }
  ticking = true;
  const asked = unit;
  try {
    const s = await getJson("/api/snapshot" + liveQuery());
    // Another receiver was chosen while this one was being read: its
    // numbers must not be painted under the new one's name.
    if (asked !== unit) return;
    if (s.error) {
      lost("no daemon", s.error);
      linkIs(false);
      return;
    }
    // Kept for the next page in this tab, so moving between pages does
    // not blank the strip while the first poll is in flight.  Per tab,
    // per receiver, and not shared: it is a paint, not a record.
    try {
      sessionStorage.setItem(`snapshot:${unit}`, JSON.stringify(s));
    } catch (e) {
      // A browser refusing storage costs the flash, nothing else.
    }
    render(s, false);
    linkIs(s.freshness !== "Disconnected");
  } finally {
    ticking = false;
    if (tickAgain) {
      tickAgain = false;
      tick();
    }
  }
}

// Paint the strip.  `cached` says the snapshot came from the last page
// rather than from the daemon just now: the values are still the last
// ones seen, and the age beside them still counts up honestly, but the
// receiver may have stopped in between -- so the state pill says where
// the numbers came from rather than asserting a link nobody has
// checked since the page loaded.
function render(s, cached) {
  const state = cached ? ["seen", "LAST SEEN"]
              : s.freshness === "Live" ? ["live", "LIVE"]
              : s.freshness === "Stale" ? ["stale", "STALE"]
              : ["down", "DISCONNECTED"];
  // Age of the slowest group of fields: the honest headline, since the
  // rest of this strip is only as current as its least current part.
  const ages = ["fast", "medium", "slow"]
    .map((t) => s.polled?.[t]?.at)
    .filter(Boolean)
    .map((at) => (Date.now() - Date.parse(at)) / 1000);
  const oldest = ages.length ? Math.max(...ages) : null;

  // The receiver's own clock, not the browser's.  Shown as of the last
  // fast poll rather than ticking: a clock animated between polls
  // would be the page's arithmetic presented as the receiver's time.
  const utc = s.time
    ? `${pad(s.time.hour)}:${pad(s.time.minute)}:${pad(s.time.second)}`
    : "--";
  // The date beside the time of day, corrected.  Almost every receiver
  // of this vintage is behind by whole GPS epochs, so that is the
  // normal condition rather than a fault; it used to raise a pill in
  // the strip, which meant an amber warning permanently lit on a
  // healthy instrument.  The correction is noted under the value, and
  // the raw date is in the tooltip, because the arithmetic is ours and
  // done against the host clock rather than anything the receiver said.
  //
  // Only claimed when the corrected date is actually to hand.  A
  // daemon older than this page does not send one, and labeling its
  // raw date as corrected would be the one thing worse than showing
  // the raw date: saying it has been fixed when it has not.
  const epochs = s.date_corrected ? (s.date?.rollover?.epochs ?? 0) : 0;
  const dateKey = epochs
    ? `date, +${epochs} GPS epoch${epochs === 1 ? "" : "s"}`
    : s.date?.rollover
      ? "date, as the receiver reports it"
      : "date";
  const stats = [
    ["receiver UTC", utc],
    [dateKey, s.date_corrected ?? s.date?.raw ?? "--", s.date?.raw],
    ["mode", s.mode ?? "--"],
    ["TFOM / FFOM", `${s.tfom ?? "--"} / ${s.ffom ?? "--"}`],
    ["EFC", s.efc === null || s.efc === undefined ? "--" : fmt(s.efc, 3) + "%"],
    ["EFC raw", s.efc_raw ?? "--"],
    ["internal temp", s.temperature_c == null ? "--" : fmt(s.temperature_c, 2) + " C"],
    ["1 PPS TI", s.time_interval_ns === null ? "--" : fmt(s.time_interval_ns, 1) + " ns"],
    // Both numbers, each said in full.  "6 of 7 up" under the word
    // "tracking" left the second number to be guessed at, and the one
    // that matters when a receiver is struggling is the difference.
    ["satellites", s.tracking == null ? "--" :
      s.visible == null
        ? `${s.tracking} tracked`
        : `${s.tracking} tracked / ${s.visible} visible`],
  ];
  // Each reading with the column it is, so what the unit does not
  // measure is left out rather than shown as a permanent "--".
  const column = {
    "TFOM / FFOM": "tfom", EFC: "efc_percent", "EFC raw": "efc_dac",
    "internal temp": "temperature_c", "1 PPS TI": "time_interval_s", satellites: "tracking",
  };
  const shown = stats.filter(([k]) => !column[k] || measures(column[k]));
  $("status").innerHTML =
    `<span class="pill ${state[0]}">${state[1]}</span>` +
    (oldest === null
      ? ""
      : `<span class="muted" id="age">oldest field ${Math.max(0, oldest).toFixed(0)}s</span>`) +
    shown
      .map(
        ([k, v, hint]) =>
          `<span class="stat"${hint ? ` title="the receiver reports ${esc(hint)}"` : ""}>` +
          `<span class="v">${esc(v)}</span><span class="k">${esc(k)}</span></span>`,
      )
      .join("") +
    (s.hardware_faults?.length
      ? `<span class="pill down">${s.hardware_faults.map(esc).join(", ")}</span>`
      : "") +
    // The receiver's own alarm, which is latched and stays latched
    // until cleared at the front panel.  Named separately where it is
    // a clock step, since that invalidates the measurements either
    // side of it and is not just another fault.
    (s.time_reset
      ? `<span class="pill down">receiver reset its clock to match GPS</span>`
      : "") +
    (s.alarming && !s.time_reset
      ? `<span class="pill stale">receiver alarm: ${(s.alarm_summary ?? []).map(esc).join(", ")}</span>`
      : "") +
    "";
}

// Nothing is known any more.
function lost(what, why) {
  $("status").innerHTML =
    `<span class="pill down">${esc(what)}</span>` +
    (why ? ` <span class="muted">${esc(why)}</span>` : "");
}

// Paint the strip for the cached snapshot of the receiver the address
// names, if this tab has one.
function paintCached() {
  const asked = new URLSearchParams(location.search).get("receiver");
  try {
    const last = sessionStorage.getItem(`snapshot:${asked}`);
    if (last) render(JSON.parse(last), true);
  } catch (e) {
    // No cache, or unreadable: the strip just starts empty.
  }
}

// ------------------------------------------------------------ lifecycle
//
// Each page's content -- charts, a stability curve, a status screen --
// is described once, to `content`, and read by this, so that every page
// does the same thing at the same moments:
//
//   - at load, and then on a timer: for a page with a range, as its
//     refresh picker says (below); for one without, every `every` ms
//     while its `auto()` says so;
//   - on a change of receiver: cleared at once, then read;
//   - on a change the page makes itself, like a new range: read, with
//     any read in flight abandoned;
//   - when the link goes: content read from the daemon is cleared,
//     since it describes a state that has ended; content read from the
//     logs is read again, since what it says still stands;
//   - when the link comes back: read.
//
// A read is never started behind another for the same receiver: one
// asked for while one is in flight runs when that one ends.  Anything
// abandoned is dropped when it arrives, however late.

// The page's description:
//   clear(why, bad)  blank what the page shows and say why, in its own
//                    place; `bad` for an error or a lost link
//   load(signal)     read the chosen receiver's content with `ask` and
//                    draw it; throws on failure
//   daemon           the content comes from the daemon, not the logs
//   every, auto()    the refresh period, and whether it applies now,
//                    for a page without a range
//   floor            for a page with one, the shortest period its
//                    refresh picker offers, in seconds
//   chosen()         optional: set up anything that depends on which
//                    receiver it is, such as what it measures; called
//                    once the receiver is known and on every change
let page = null;
let running = null;
let again = false;

// `asked` is set for a read the reader asked for by hand, which is
// tried even while the link is down: they may know better.
function refresh(asked = false) {
  if (running) {
    again = true;
    return;
  }
  // Cleared when the link went; read again when it comes back.
  if (page.daemon && linkUp === false && !asked) return;
  const control = new AbortController();
  running = control;
  readStarted();
  page
    .load(control.signal)
    .catch((e) => {
      if (!(e instanceof Superseded)) page.clear(e.message, true);
    })
    .finally(() => {
      if (running !== control) return;
      running = null;
      if (again) {
        again = false;
        refresh();
      }
    });
}

// Abandon whatever is in flight and read again.
function renew(asked = false) {
  running?.abort();
  running = null;
  again = false;
  refresh(asked);
}

// Clear first, then read: what is shown is about to be wrong.
function restart(why, bad = false) {
  running?.abort();
  running = null;
  again = false;
  page.clear(why, bad);
  refresh();
}

function linkIs(up) {
  if (up === linkUp) return;
  const was = linkUp;
  linkUp = up;
  // The first answer for a receiver is not a change.
  if (was === null) return;
  listReceivers();
  if (up) {
    if (page.daemon) restart(`reading ${unitName(unit)}...`);
    else refresh();
  } else if (page.daemon) {
    running?.abort();
    running = null;
    again = false;
    page.clear("the link is down", true);
  } else {
    refresh();
  }
}

function switchTo(serial) {
  unit = serial;
  carryUnit();
  linkUp = null;
  // The strip too: the last unit's numbers are not this one's.
  $("status").innerHTML = `<span class="muted">reading ${esc(unitName(unit))}...</span>`;
  paintCached();
  listReceivers();
  tick();
  page.chosen?.();
  restart(`reading ${unitName(unit)}...`);
}

// Register the page's content and start it.  Every page calls this
// once, last.
async function content(spec) {
  page = spec;
  // The refresh picker's choices depend on the page's floor.
  refreshChoice = floored(refreshChoice);
  if (rangeEl) rememberRefresh();
  drawRangeControl();
  colophon();
  paintCached();
  await listReceivers();
  carryUnit();
  $("unit").onchange = () => switchTo($("unit").value);
  page.chosen?.();
  tick();
  setInterval(tick, STRIP_EVERY);
  setInterval(listReceivers, RECEIVERS_EVERY);
  if (page.every) {
    setInterval(() => {
      if (page.auto()) refresh();
    }, page.every);
  }
  refresh();
  schedule();
}

// ------------------------------------------------------------ colophon

// Whose this is, under what license, and where its source and issue
// tracker are, from the server's own package metadata.  Left empty if
// the server cannot say.
async function colophon() {
  const about = await getJson("/api/about");
  if (about.error) return;
  const link = (href, text) => `<a href="${esc(href)}">${esc(text)}</a>`;
  $("colophon").innerHTML = [
    `© ${esc(about.copyright)}`,
    esc(about.license),
    `smartclock-web ${esc(about.version)}`,
    link(about.repository, "source"),
    link(`${about.repository}/issues`, "issues"),
  ].join(" · ");
}

// ---------------------------------------------------------- time range
//
// The range control works as Grafana's dashboards do:
//
//   - a range is "the last N units" up to now, moving with the clock,
//     or a fixed pair; a drag on a chart zooms to a fixed pair;
//   - the back and forward buttons move it by half its length; zoom
//     out doubles it about its center, as a double click on a chart
//     does; the keys are `t Left`, `t Right`, `t -`, `t +` (halve it)
//     and `t a` (fix it where it is), and Ctrl+Z zooms out too;
//   - each change of range is a step the browser's Back button undoes;
//   - a refresh picker, Auto unless chosen otherwise, says how often
//     the range is read again.
//
// Two departures.  A range moved or zoomed out to end within half its
// length of now becomes the moving range of its length, where Grafana's
// dashboards slide on into a future with no readings.  And a fixed
// range is read again only until a read has started after its end;
// past that its readings cannot change.
//
// The range rides in the address (`last=SECONDS`, `last=all`, or
// `from=..&to=..` in unix seconds) so a reload or a shared link shows
// the same window.  Grafana's forms are read too -- `from=now-6h&to=now`,
// epoch milliseconds, ISO times -- and rewritten as these, as is the
// older `range=SECONDS`.

const RANGE_UNITS = [["min", 60], ["h", 3600], ["d", 86400]];
const RANGE_PRESETS = [
  ["1h", 3600], ["6h", 21600], ["24h", 86400], ["48h", 172800],
  ["7d", 604800], ["30d", 2592000],
];
// The address keys that hold the range.
const RANGE_KEYS = ["last", "from", "to", "range"];
// The shortest window a zoom makes, in seconds.
const SHORTEST_WINDOW = 60;
// What a zoom out multiplies the window by, and a zoom in divides it by.
const ZOOM = 2;
// The fraction of the window a step moves it by, and how near now a
// window must end to become the moving one.
const STEP = 0.5;

// `last`: seconds, or "all"; `from`/`to`: unix seconds when absolute.
let range = { last: 3600, from: null, to: null };
// The page's default length, in seconds.
let rangeFallback = 3600;

// Grafana's relative instants: "now", or "now-" a count and a unit.
// Its calendar units (months, years) and rounding (`now/d`) are not
// read.
const RELATIVE = /^now(?:-(\d+)([smhdw]))?$/;
const RELATIVE_UNITS = { s: 1, m: 60, h: 3600, d: 86400, w: 604800 };
// A bare number above this is epoch milliseconds, as Grafana writes,
// and below it unix seconds, as this page does: 1e11 seconds is the
// year 5138, 1e11 milliseconds 1973.
const MILLISECONDS_ABOVE = 1e11;

// An instant from the address: `{ ago }` in seconds before now, or
// `{ at }` in unix seconds; null if it is neither.
function instant(text) {
  if (text === null) return null;
  const relative = RELATIVE.exec(text);
  if (relative) {
    return { ago: relative[1] ? Number(relative[1]) * RELATIVE_UNITS[relative[2]] : 0 };
  }
  if (/^\d+(\.\d+)?$/.test(text)) {
    const n = Number(text);
    return { at: n > MILLISECONDS_ABOVE ? n / 1000 : n };
  }
  const parsed = Date.parse(text);
  return Number.isFinite(parsed) ? { at: parsed / 1000 } : null;
}

// Read the range from the address, and write it back in this page's
// own form.
function readRange(fallback) {
  rangeFallback = fallback ?? rangeFallback;
  range = rangeFrom(new URLSearchParams(location.search));
  rememberRange();
}

function rangeFrom(v) {
  const from = instant(v.get("from")), to = instant(v.get("to"));
  if (from && to) {
    const now = Date.now() / 1000;
    // Up to now is a moving range, however its start is written.
    const length = from.ago ?? now - from.at;
    if (to.ago === 0 && length > 0) return { last: Math.round(length), from: null, to: null };
    const start = from.at ?? now - from.ago, end = to.at ?? now - to.ago;
    if (end > start) return { last: null, from: start, to: end };
  }
  const last = v.get("last") ?? v.get("range");
  if (last === "all" || last === "null") return { last: "all", from: null, to: null };
  if (last && Number(last) > 0) return { last: Number(last), from: null, to: null };
  return { last: rangeFallback, from: null, to: null };
}

// The bounds to ask the server for, in unix seconds; null is open.
function rangeBounds() {
  if (range.last === "all") return { from: 0, to: null, seconds: null };
  if (range.last !== null) {
    const to = Date.now() / 1000;
    return { from: to - range.last, to, seconds: range.last };
  }
  return { from: range.from, to: range.to, seconds: range.to - range.from };
}

// Whether the range moves with the clock.
const moving = () => range.last !== null;
// Whether reading the range again can show anything new: it moves, or
// a fixed one, such as a note's, ends after the last read started.
const growing = () => moving() || range.to > readAt / 1000;

function rememberRange(push = false) {
  const fixed = !moving();
  remember({
    last: fixed ? null : range.last,
    from: fixed ? Math.round(range.from) : null,
    to: fixed ? Math.round(range.to) : null,
    range: null,
  }, push);
  carry();
}

// A length in seconds as the biggest whole unit that divides it.
function splitLength(secs) {
  for (const [name, size] of [...RANGE_UNITS].reverse()) {
    if (secs >= size && Number.isInteger(secs / size)) return [secs / size, size];
  }
  return [Math.max(1, Math.round(secs / 60)), 60];
}

function shortTime(t) {
  const d = new Date(t * 1000);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ` +
    `${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

// The control's element and what the page does on a change, kept so
// that a change from anywhere -- the control, a drag, a key, Back --
// is drawn and read the same way.
let rangeEl = null;
let rangeChanged = null;

// Build the control into `el`; `changed` is called after every change.
function rangeControl(el, changed) {
  rangeEl = el;
  rangeChanged = changed;
  refreshChoice = refreshFrom(new URLSearchParams(location.search));
  drawRangeControl();
}

// Make `next` the range: into the address as a step Back undoes, onto
// the control, and read.  Charts synced by cursor each report the same
// drag, so a range that is already the one shown is not a change.
function setRange(next) {
  if (next.last === range.last && next.from === range.from && next.to === range.to) return;
  range = next;
  rememberRange(true);
  rangeShown();
}

// Draw the control for the range now set, and read it.
function rangeShown() {
  drawRangeControl();
  schedule();
  rangeChanged?.();
}

// A fixed window, from a drag, a note or a step.
function fixRange(from, to) {
  if (to > from) setRange({ last: null, from, to });
}

// The moving window of `length` seconds.
function moveWith(length) {
  setRange({ last: Math.max(SHORTEST_WINDOW, Math.round(length)), from: null, to: null });
}

// Back to a window moving with the clock, of the length shown.
function liveAgain() {
  moveWith(rangeBounds().seconds || rangeFallback);
}

// A window that would end within a step of now, or later, is the moving
// window of its length; an earlier one stays fixed.
function settle(from, to) {
  const length = to - from;
  if (to >= Date.now() / 1000 - length * STEP) moveWith(length);
  else fixRange(from, to);
}

// Move by a step, earlier (-1) or later (1).
function step(direction) {
  if (range.last === "all") return;
  const { from, to, seconds } = rangeBounds();
  const by = direction * seconds * STEP;
  if (direction > 0) settle(from + by, to + by);
  else fixRange(from + by, to + by);
}

// Scale the window about its center: ZOOM zooms out, 1 / ZOOM in.
function zoom(factor) {
  if (range.last === "all") return;
  const { from, to, seconds } = rangeBounds();
  const half = Math.max(SHORTEST_WINDOW, seconds * factor) / 2;
  const center = (from + to) / 2;
  if (factor > 1) settle(center - half, center + half);
  else fixRange(center - half, center + half);
}

const zoomOut = () => zoom(ZOOM);
const zoomIn = () => zoom(1 / ZOOM);

// Fix a moving window where it is now.
function fixWhereItIs() {
  if (!moving() || range.last === "all") return;
  const { from, to } = rangeBounds();
  fixRange(from, to);
}

// The window shown, as times, for a span that is always present, so the
// right-aligned buttons after it stay where they are.
function shownSpan() {
  if (range.last === "all") return "all";
  const { from, to } = rangeBounds();
  return `${shortTime(from)} – ${shortTime(to)}`;
}

function drawRangeControl() {
  const el = rangeEl;
  if (!el) return;
  const relative = moving();
  const all = range.last === "all";
  const [n, size] = relative && !all ? splitLength(range.last) : [1, 3600];
  const disabled = (off) => (off ? " disabled" : "");
  el.innerHTML =
    RANGE_PRESETS.map(([label, secs]) =>
      `<button data-last="${secs}" aria-pressed="${range.last === secs}">${label}</button>`).join(" ") +
    ` <button data-last="all" aria-pressed="${all}">all</button>` +
    ` <label>last <input id="range-n" type="number" min="1" step="1" value="${n}" ` +
    `style="width:5em"${disabled(!relative)}> ` +
    `<select id="range-unit"${disabled(!relative)}>` +
    RANGE_UNITS.map(([name, s]) =>
      `<option value="${s}"${s === size ? " selected" : ""}>${name}</option>`).join("") +
    `</select></label>` +
    ` <span id="range-shown" class="muted">${esc(shownSpan())}</span>` +
    ` <span class="range-group">` +
    `<button id="range-back" class="glyph" title="earlier by half the window (t ←)"${disabled(all)}>&lsaquo;</button>` +
    `<button id="range-out" class="glyph" title="zoom out (t -, Ctrl+Z, double click)"${disabled(all)}>−</button>` +
    `<button id="range-forward" class="glyph" title="later by half the window (t →)"${disabled(all || relative)}>&rsaquo;</button>` +
    `<button id="range-now" title="the same length, up to now"${disabled(relative)}>now</button>` +
    `</span> <span class="range-group">` +
    `<button id="range-reload" class="glyph" title="read again now">⟳</button>` +
    `<select id="range-refresh" title="${esc(refreshTitle())}"${disabled(!growing())}>` +
    refreshOptions().map(([value, label]) =>
      `<option value="${value}"${value === refreshChoice ? " selected" : ""}>${label}</option>`).join("") +
    `</select></span>`;
  for (const b of el.querySelectorAll("button[data-last]")) {
    b.onclick = () =>
      setRange({ last: b.dataset.last === "all" ? "all" : Number(b.dataset.last), from: null, to: null });
  }
  const setLast = () => {
    const count = Number($("range-n").value), unitSize = Number($("range-unit").value);
    if (count > 0) setRange({ last: count * unitSize, from: null, to: null });
  };
  $("range-n").onchange = setLast;
  $("range-unit").onchange = setLast;
  $("range-back").onclick = () => step(-1);
  $("range-out").onclick = () => zoomOut();
  $("range-forward").onclick = () => step(1);
  $("range-now").onclick = liveAgain;
  $("range-reload").onclick = () => renew(true);
  $("range-refresh").onchange = () => {
    refreshChoice = $("range-refresh").value;
    rememberRefresh();
    drawRangeControl();
    schedule();
  };
}

// Grafana's time keys: `t`, then the second key within this many ms.
const CHORD = 1000;
// The second key, by its `key`, and what it does.
const TIME_KEYS = {
  ArrowLeft: () => step(-1),
  ArrowRight: () => step(1),
  "-": zoomOut,
  "+": zoomIn,
  // The + key unshifted, as Grafana takes it.
  "=": zoomIn,
  a: fixWhereItIs,
};
// Keys that only modify the next, and so neither start nor end a chord.
const MODIFIERS = new Set(["Shift", "Control", "Alt", "Meta"]);
// When `t` was pressed, as the event's timeStamp; -Infinity when no
// chord is open.
let chordAt = -Infinity;
addEventListener("keydown", (e) => {
  if (!rangeEl || e.defaultPrevented || MODIFIERS.has(e.key)) return;
  // Keys typed into a control are the control's: arrows move a select
  // and `t` is text.
  if (e.target.closest?.("input, select, textarea, [contenteditable]")) return;
  if (e.ctrlKey && !e.altKey && !e.metaKey && e.key === "z") {
    e.preventDefault();
    zoomOut();
    return;
  }
  if (e.ctrlKey || e.altKey || e.metaKey) return;
  const action = TIME_KEYS[e.key];
  if (action && e.timeStamp - chordAt <= CHORD) {
    e.preventDefault();
    chordAt = -Infinity;
    action();
    return;
  }
  chordAt = e.key === "t" ? e.timeStamp : -Infinity;
});

// Back and Forward: the range from the address returned to, and
// everything else -- the receiver, the columns -- as it is now, since
// only a change of range is a step.
addEventListener("popstate", () => {
  if (!rangeEl) return;
  const returned = new URLSearchParams(location.search);
  const q = new URLSearchParams(remembered);
  for (const k of RANGE_KEYS) {
    if (returned.has(k)) q.set(k, returned.get(k));
    else q.delete(k);
  }
  range = rangeFrom(q);
  const s = q.toString();
  history.replaceState(null, "", location.pathname + (s ? `?${s}` : ""));
  remembered = location.search;
  rememberRange();
  rangeShown();
});

// ------------------------------------------------------------- refresh
//
// How often the range is read again.  Auto, as Grafana's does, reads
// about once per pixel's worth of time across the window, here rounded
// up to the next of Grafana's intervals; never more often than the
// page's floor, which is what one read of it costs.  Paused while the
// tab is hidden, and put off while a read is still running or the
// pointer is on a chart or text is selected, since each read redraws
// the page and would take the cursor readout, a drag in progress or the
// selection with it.

const REFRESH_INTERVALS = [
  ["5s", 5], ["10s", 10], ["30s", 30], ["1m", 60], ["5m", 300], ["15m", 900],
  ["30m", 1800], ["1h", 3600], ["2h", 7200], ["1d", 86400],
];
const REFRESH_OFF = "off";
const REFRESH_AUTO = "auto";
const REFRESH_DEFAULT = REFRESH_AUTO;
// The window Auto assumes for `all`, whose length the page does not
// know: the longest preset.
const ALL_SPAN = RANGE_PRESETS[RANGE_PRESETS.length - 1][1];
// How long a refresh put off waits to try again, in ms.
const PUT_OFF_WAIT = 1000;

// "off", "auto", or one of REFRESH_INTERVALS' names.
let refreshChoice = REFRESH_DEFAULT;
// The pending timer for the next read, if any.
let refreshTimer = null;
// When the last read started, in ms since the epoch.
let readAt = Date.now();

// The page's floor in seconds, or null on a page without a range.
const refreshFloor = () => page?.floor ?? null;

// An interval's length in seconds, by its name; undefined for Off and Auto.
const refreshSeconds = (name) => REFRESH_INTERVALS.find(([n]) => n === name)?.[1];

function refreshFrom(v) {
  const asked = v.get("refresh");
  const known = [REFRESH_OFF, REFRESH_AUTO, ...REFRESH_INTERVALS.map(([name]) => name)];
  return known.includes(asked) ? asked : REFRESH_DEFAULT;
}

function rememberRefresh() {
  remember({ refresh: refreshChoice === REFRESH_DEFAULT ? null : refreshChoice });
}

// A choice faster than the page's floor, from an address made on
// another page, as the fastest it does offer: the picker cannot show
// what it does not offer.
function floored(choice) {
  const floor = refreshFloor() ?? 0;
  const secs = refreshSeconds(choice);
  if (secs === undefined || secs >= floor) return choice;
  return REFRESH_INTERVALS.find(([, s]) => s >= floor)[0];
}

// The choices the picker offers: none faster than the page's floor.
function refreshOptions() {
  const floor = refreshFloor() ?? 0;
  return [
    [REFRESH_OFF, "Off"],
    [REFRESH_AUTO, "Auto"],
    ...REFRESH_INTERVALS.filter(([, secs]) => secs >= floor).map(([name]) => [name, name]),
  ];
}

// The period in seconds the range is read again at, or null for never.
function refreshEvery() {
  const floor = refreshFloor();
  if (floor === null || !growing() || refreshChoice === REFRESH_OFF) return null;
  if (refreshChoice !== REFRESH_AUTO) return Math.max(floor, refreshSeconds(refreshChoice));
  const span = rangeBounds().seconds ?? ALL_SPAN;
  const wanted = Math.max(floor, span / Math.max(1, innerWidth));
  return (REFRESH_INTERVALS.find(([, secs]) => secs >= wanted) ?? REFRESH_INTERVALS.at(-1))[1];
}

function refreshTitle() {
  if (!growing()) return "a range now past is not read again";
  const every = refreshEvery();
  return every === null ? "not read again" : `read again every ${every} s`;
}

// Set the timer for the next read, replacing any set before.
function schedule() {
  clearTimeout(refreshTimer);
  const every = refreshEvery();
  if (every === null) return;
  refreshTimer = setTimeout(due, every * 1000);
}

function due() {
  if (document.hidden) return;
  if (running || document.querySelector(".u-over:hover") || String(getSelection() ?? "")) {
    refreshTimer = setTimeout(due, PUT_OFF_WAIT);
    return;
  }
  refresh();
}

// A read has started, whatever started it: the next is timed from it,
// and the control's times and picker follow.
function readStarted() {
  readAt = Date.now();
  schedule();
  const shownEl = $("range-shown");
  if (shownEl) shownEl.textContent = shownSpan();
  const picker = $("range-refresh");
  if (picker) {
    picker.disabled = !growing();
    picker.title = refreshTitle();
  }
}

// A hidden tab is not read; on its return it is, once a read is due.
document.addEventListener("visibilitychange", () => {
  const every = refreshEvery();
  if (document.hidden || every === null) return;
  clearTimeout(refreshTimer);
  refreshTimer = setTimeout(due, Math.max(0, readAt + every * 1000 - Date.now()));
});
