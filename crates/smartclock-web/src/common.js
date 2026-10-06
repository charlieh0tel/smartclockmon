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
// goes through here, so none of them wipes another's setting.
function remember(params) {
  const q = new URLSearchParams(location.search);
  for (const [k, v] of Object.entries(params)) {
    if (v === null || v === undefined) q.delete(k);
    else q.set(k, String(v));
  }
  const s = q.toString();
  history.replaceState(null, "", location.pathname + (s ? `?${s}` : ""));
}

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
//   - at load, and every `every` ms while the page's `auto()` says so;
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
//   every, auto()    the refresh period, and whether it applies now
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
// A range is a pair (from, to), and most of the time it is "the last
// N units up to now": relative, moving with the clock.  Dragging on a
// chart makes it absolute -- a fixed pair -- and stepping it back and
// forth keeps it so; "now" returns to relative with the same length.
// The presets only fill the "last" box in.  The whole thing lives in
// the address (`last=SECONDS`, `last=all`, or `from=..&to=..`) so a
// reload or a shared link shows the same window; the older
// `range=SECONDS` is still read.

const RANGE_UNITS = [["min", 60], ["h", 3600], ["d", 86400]];
const RANGE_PRESETS = [
  ["1h", 3600], ["6h", 21600], ["24h", 86400], ["48h", 172800],
  ["7d", 604800], ["30d", 2592000],
];

// `last`: seconds, or "all"; `from`/`to`: unix seconds when absolute.
let range = { last: 3600, from: null, to: null };
// The page's default length, in seconds.
let rangeFallback = 3600;

function readRange(fallback) {
  rangeFallback = fallback ?? rangeFallback;
  const v = new URLSearchParams(location.search);
  if (v.has("from") && v.has("to")) {
    const from = Number(v.get("from")), to = Number(v.get("to"));
    if (Number.isFinite(from) && Number.isFinite(to) && to > from) {
      range = { last: null, from, to };
      return;
    }
  }
  const last = v.get("last") ?? v.get("range");
  if (last === "all" || last === "null") range = { last: "all", from: null, to: null };
  else if (last && Number(last) > 0) range = { last: Number(last), from: null, to: null };
  else range = { last: rangeFallback, from: null, to: null };
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

function rememberRange() {
  if (range.last !== null) remember({ last: range.last, from: null, to: null });
  else remember({ last: null, from: Math.round(range.from), to: Math.round(range.to) });
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
// that a change from anywhere -- the control, a drag, a note -- is
// drawn and read the same way.
let rangeEl = null;
let rangeChanged = null;

// Build the control into `el`; `changed` is called after every change.
function rangeControl(el, changed) {
  rangeEl = el;
  rangeChanged = changed;
  drawRangeControl();
}

// Make `next` the range: into the address, onto the control, and read.
// Charts synced by cursor each report the same drag, so a range that is
// already the one shown is not a change.
function setRange(next) {
  if (next.last === range.last && next.from === range.from && next.to === range.to) return;
  range = next;
  rememberRange();
  drawRangeControl();
  rangeChanged?.();
}

// A fixed window, from a drag or a note.
function fixRange(from, to) {
  if (to > from) setRange({ last: null, from, to });
}

// Back to a window moving with the clock, of the length shown.
function liveAgain() {
  const { seconds } = rangeBounds();
  setRange({ last: Math.max(60, Math.round(seconds || rangeFallback)), from: null, to: null });
}

function drawRangeControl() {
  const el = rangeEl;
  const relative = range.last !== null;
  const [n, size] = relative && range.last !== "all" ? splitLength(range.last) : [1, 3600];
  el.innerHTML =
    RANGE_PRESETS.map(([label, secs]) =>
      `<button data-last="${secs}" aria-pressed="${range.last === secs}">${label}</button>`).join(" ") +
    ` <button data-last="all" aria-pressed="${range.last === "all"}">all</button>` +
    ` <label>last <input id="range-n" type="number" min="1" step="1" value="${n}" ` +
    `style="width:5em"${relative ? "" : " disabled"}> ` +
    `<select id="range-unit"${relative ? "" : " disabled"}>` +
    RANGE_UNITS.map(([name, s]) =>
      `<option value="${s}"${s === size ? " selected" : ""}>${name}</option>`).join("") +
    `</select></label>` +
    (relative ? "" :
      ` <span class="muted">${esc(shortTime(range.from))} – ${esc(shortTime(range.to))}</span>` +
      ` <button id="range-back" title="earlier by one window">&lsaquo;</button>` +
      ` <button id="range-fwd" title="later by one window">&rsaquo;</button>` +
      ` <button id="range-now" title="the same length, up to now">now</button>`);
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
  if (!relative) {
    const len = range.to - range.from;
    $("range-back").onclick = () => fixRange(range.from - len, range.to - len);
    $("range-fwd").onclick = () => fixRange(range.from + len, range.to + len);
    $("range-now").onclick = liveAgain;
  }
}
