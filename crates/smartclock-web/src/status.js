// The status strip, and the few helpers it needs.
//
// Shared by every page rather than living on the front one: the state
// of the receiver is the context for whatever else a page is showing,
// and a sky plot or a stability curve read without knowing the unit is
// in holdover is read wrongly.  Loaded as a classic script before each
// page's own, so these are ordinary globals.

const $ = (id) => document.getElementById(id);
const pad = (n) => String(n).padStart(2, "0");
// Total: never rejects, never hangs.  A rejected fetch anywhere in the
// boot sequence used to abort it before the polling timer was
// installed, leaving the page stuck on "connecting..." until someone
// reloaded it -- the one failure that needed a human.
async function getJson(url) {
  try {
    const response = await fetch(url, { signal: AbortSignal.timeout(5000) });
    return await response.json();
  } catch (e) {
    return { error: String(e) };
  }
}
// Everything interpolated into innerHTML goes through this.  The values
// are numbers and enums today, but they come from a string scraper over
// a serial line, and one field becoming an Option<String> upstream
// would otherwise make this page execute whatever the receiver said.
const esc = (v) =>
  String(v).replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const fmt = (v, digits = 3) =>
  v === null || v === undefined ? "--" : Number(v).toFixed(digits);

// ---------------------------------------------------------------- status

let ticking = false;
async function tick() {
  if (ticking) return;
  ticking = true;
  try {
    await poll();
  } finally {
    ticking = false;
  }
}

async function poll() {
  const s = await getJson("/api/snapshot");
  if (s.error) {
    lost("no daemon", s.error);
    return;
  }
  // Kept for the next page in this tab, so moving between pages does
  // not blank the strip while the first poll is in flight.  Per tab and
  // not shared: it is a paint, not a record.
  try {
    sessionStorage.setItem("snapshot", JSON.stringify(s));
  } catch (e) {
    // A browser refusing storage costs the flash, nothing else.
  }
  render(s, false);
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
  // daemon older than this page does not send one, and labelling its
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
  $("status").innerHTML =
    `<span class="pill ${state[0]}">${state[1]}</span>` +
    (oldest === null
      ? ""
      : `<span class="muted" id="age">oldest field ${Math.max(0, oldest).toFixed(0)}s</span>`) +
    stats
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

// ------------------------------------------------------------- receiver

// Which receiver the pages that read the log are about.  Null until
// `chooseReceiver` has run, and then always set: the server defaults to
// the unit seen most recently, and pinning that choice keeps a later
// reload of one panel from disagreeing with another.
let unit = null;

function withUnit(q) {
  if (unit !== null) q.set("receiver", unit);
  return q;
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

// Carry the chosen receiver on the links between pages, so moving from
// history to stability keeps the unit rather than falling back to the
// newest.  In the address too, so a reload or a shared link does.
function carryUnit() {
  const q = unit === null ? "" : `?receiver=${encodeURIComponent(unit)}`;
  for (const a of document.querySelectorAll("nav a")) {
    a.search = q;
  }
  remember({ receiver: unit });
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

function readRange(fallback) {
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
  else range = { last: fallback ?? 3600, from: null, to: null };
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

// Build the control into `el`; `changed` is called after every change.
function rangeControl(el, changed) {
  const apply = () => { rememberRange(); rangeControl(el, changed); changed(); };
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
    b.onclick = () => {
      range = { last: b.dataset.last === "all" ? "all" : Number(b.dataset.last), from: null, to: null };
      apply();
    };
  }
  const setLast = () => {
    const count = Number($("range-n").value), unitSize = Number($("range-unit").value);
    if (!(count > 0)) return;
    range = { last: count * unitSize, from: null, to: null };
    apply();
  };
  $("range-n").onchange = setLast;
  $("range-unit").onchange = setLast;
  if (!relative) {
    const len = range.to - range.from;
    $("range-back").onclick = () => { range = { last: null, from: range.from - len, to: range.to - len }; apply(); };
    $("range-fwd").onclick = () => { range = { last: null, from: range.from + len, to: range.to + len }; apply(); };
    $("range-now").onclick = () => { range = { last: Math.max(60, Math.round(len)), from: null, to: null }; apply(); };
  }
}

// A drag on a chart: fix the window.
function setAbsolute(from, to) {
  if (!(to > from)) return;
  range = { last: null, from, to };
  rememberRange();
}

// Fill the receiver selector, and call `changed` when it changes.
//
// The unit comes from the address when it names one the log holds, and
// is otherwise the one seen most recently.  The bar is hidden for a log
// with one unit, or none: a control whose only option is the one
// already chosen is noise.  Shown the moment a second unit appears in
// the log, which is when the plots would otherwise start interleaving
// two oscillators without saying so.
async function chooseReceiver(changed) {
  const list = await getJson("/api/receivers");
  if (list.error || !Array.isArray(list) || list.length === 0) return;
  const asked = new URLSearchParams(location.search).get("receiver");
  unit = (list.find((r) => String(r.id) === asked) ?? list[0]).id;
  carryUnit();
  if (list.length < 2) return;
  const select = $("unit");
  select.innerHTML = list
    .map((r) => {
      const name = [r.model, r.serial].filter(Boolean).join(" ");
      return `<option value="${r.id}">${esc(name || `receiver ${r.id}`)}</option>`;
    })
    .join("");
  select.value = String(unit);
  const seen = () => {
    const r = list.find((x) => String(x.id) === select.value);
    $("unit-seen").textContent = r ? `last seen ${r.last_seen.slice(0, 19)}Z` : "";
  };
  seen();
  select.onchange = () => {
    unit = Number(select.value);
    carryUnit();
    seen();
    changed();
  };
  $("unit-bar").hidden = false;
}

// Every page carries the strip, so every page keeps it current.
addEventListener("DOMContentLoaded", () => {
  // A page with no selector of its own still passes the unit along, so
  // a detour through the sky does not drop it.
  const asked = new URLSearchParams(location.search).get("receiver");
  if (asked !== null) {
    for (const a of document.querySelectorAll("nav a")) {
      a.search = `?receiver=${encodeURIComponent(asked)}`;
    }
  }
  if (!$("status")) return;
  try {
    const last = sessionStorage.getItem("snapshot");
    if (last) render(JSON.parse(last), true);
  } catch (e) {
    // No cache, or unreadable: the strip just starts empty.
  }
  tick();
  setInterval(tick, 1000);
});
