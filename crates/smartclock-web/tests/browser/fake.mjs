// A fake of smartclock-web's API, installed on a Playwright page.
//
// The pages are served by the real smartclock-web; only /api/* is
// answered here, from answers recorded against the simulator, so a test
// can decide which receivers exist, when a daemon goes, and how long an
// answer takes -- the edges the pages have to handle alike, and which a
// live daemon cannot be made to show on demand.
//
// Each receiver's answers carry its serial where a page shows it, so a
// test can tell whose data is on screen.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const fixture = (name) => JSON.parse(readFileSync(join(here, "fixtures", `${name}.json`), "utf8"));
const RECORDED = Object.fromEntries(
  ["receivers", "snapshot", "info", "journal", "facts", "history", "adev", "status", "about"].map((n) => [n, fixture(n)]),
);
const UPLOT = join(here, "node_modules", "uplot", "dist");

// Endpoints answered by the daemon, which fail while it is gone; the
// rest are read from the logs and do not.
const FROM_DAEMON = new Set(["snapshot", "info", "status"]);

// One simulated receiver.
//   serial   its serial number, which its answers carry
//   up       whether its daemon is attached
//   mode     the strip's mode, so the strip says whose snapshot it is
//   present  the stability page's reading count, for the same reason
//   delay    milliseconds to hold each endpoint's answer, by name
//   columns  what it measures, as the server lists it: every column
//            the history serves, unless a test says otherwise
//   skew     seconds its history's times move, each kept inside its
//            bucket as the server's mean times are
export function receiver(serial, mode, present, columns = RECORDED.history.plottable) {
  return { serial, up: true, mode, present, delay: {}, columns, skew: 0 };
}

// `at` moved by `skew` seconds, wrapping inside the bucket each time
// falls in for the window `query` asks for, since the server's mean
// time for a bucket never leaves it.
function skewed(at, skew, query) {
  const from = Number(query.get("from")), to = Number(query.get("to"));
  const points = Number(query.get("points"));
  if (!skew || !(to > from) || !points) return at;
  const width = (to - from + 1) / points;
  return at.map((t) => {
    const start = from + Math.floor((t - from) / width) * width;
    return start + ((t - start + skew) % width);
  });
}

// The state a test drives.  `units` is the list the server reports.
export class Fake {
  constructor(units) {
    this.units = units;
    // Requests seen, by endpoint, for counting.
    this.seen = {};
    // Whether the history answers every column asked for, rather than
    // only the recorded ones.
    this.everyColumn = false;
    // The host's sensors, [{ name, quantity, unit, value }], or null for
    // a host without the sensor service.
    this.sensors = null;
  }

  unit(serial) {
    return this.units.find((u) => u.serial === serial) ?? this.units[0];
  }

  answer(endpoint, u, query = new URLSearchParams()) {
    if (FROM_DAEMON.has(endpoint) && !u.up) {
      return { error: `no daemon is attached to receiver ${u.serial}` };
    }
    const body = structuredClone(RECORDED[endpoint]);
    switch (endpoint) {
      case "receivers":
        return this.units.map((x) => ({
          ...RECORDED.receivers[0],
          serial: x.serial,
          instance: x.up ? `sim-${x.serial}` : null,
          columns: x.columns,
        }));
      case "snapshot":
        return { ...body, mode: u.mode, at: new Date().toISOString(),
          polled: Object.fromEntries(Object.keys(body.polled ?? {}).map((t) =>
            [t, { ...body.polled[t], at: new Date().toISOString() }])) };
      case "info":
        return { ...body, identity: `HEWLETT-PACKARD,58503A,${u.serial},3704-C` };
      case "status":
        body.screen.text = `screen of ${u.serial}\n${body.screen.text}`;
        return body;
      case "adev":
        return { ...body, present: u.present };
      case "history": {
        // With `everyColumn`, every column asked for: the recorded ones
        // as recorded, any other in the first recorded one's shape, so
        // a page asking for a column the recording lacks still gets a
        // chart.  Otherwise only what was recorded.
        const asked = (query.get("columns") ?? "").split(",").filter(Boolean);
        const plots = this.everyColumn && asked.length
          ? asked.map((column) => body.plots.find((p) => p.column === column) ?? { ...body.plots[0], column })
          : body.plots;
        return { ...body, plots, at: skewed(body.at, u.skew, query) };
      }
      case "sensors":
        return {
          sensors: (this.sensors ?? []).map((s) => ({
            name: s.name,
            quantity: s.quantity,
            unit: s.unit,
            source: `/sys/fake/${s.name}`,
            device: "fake",
          })),
          every_s: 10,
          first: this.sensors ? RECORDED.history.at[0] : null,
          last: this.sensors ? RECORDED.history.at.at(-1) : null,
        };
      case "sensors/history":
        // A line per sensor of the quantity over the recorded history's
        // range, at a reading every fourth of its times moved seven
        // seconds on, so they fall in buckets of their own, as a
        // sensor's ten-second readings do beside the receiver's.
        // In name order, as the server gives them.
        return (this.sensors ?? [])
          .filter((s) => s.quantity === query.get("quantity"))
          .toSorted((a, b) => a.name.localeCompare(b.name))
          .map((s) => {
            const at = RECORDED.history.at.filter((_, i) => i % 4 === 0).map((t) => t + 7);
            return { name: s.name, at, values: at.map(() => s.value) };
          });
      case "sensors/latest":
        if (!this.sensors) return { error: "no sensor service" };
        return {
          every_s: 10,
          readings: this.sensors.map((s) => ({
            name: s.name,
            quantity: s.quantity,
            unit: s.unit,
            source: `/sys/fake/${s.name}`,
            at: new Date().toISOString(),
            value: s.value,
            error: null,
            current: true,
          })),
        };
      case "notes":
        return this.answer("journal", u).notes;
      case "journal": {
        // One note, midway through the recorded history, naming whose
        // it is.
        const at = RECORDED.history.at;
        const mid = at[Math.floor(at.length / 2)];
        return { ...body, notes: [{ at: new Date(mid * 1000).toISOString(), text: `note of ${u.serial}` }] };
      }
      default:
        return body;
    }
  }

  async install(page) {
    await page.route("https://cdn.jsdelivr.net/npm/uplot@1.6.32/dist/*", (route) =>
      route.fulfill({ path: join(UPLOT, new URL(route.request().url()).pathname.split("/").pop()) }),
    );
    await page.route(/\/api\//, async (route) => {
      const url = new URL(route.request().url());
      const endpoint = url.pathname.replace("/api/", "");
      this.seen[endpoint] = (this.seen[endpoint] ?? 0) + 1;
      const u = this.unit(url.searchParams.get("receiver"));
      // With no receiver at all, only the host's own endpoints answer.
      const host = endpoint === "receivers" || endpoint.startsWith("sensors");
      if (!u && !host) {
        await route.fulfill({ contentType: "application/json", body: JSON.stringify({ error: "no receiver" }) });
        return;
      }
      const wait = host ? 0 : (u.delay[endpoint] ?? 0);
      if (wait) await new Promise((r) => setTimeout(r, wait));
      // The page may have gone on without this answer.
      await route
        .fulfill({ contentType: "application/json", body: JSON.stringify(this.answer(endpoint, u, url.searchParams)) })
        .catch(() => {});
    });
  }
}
