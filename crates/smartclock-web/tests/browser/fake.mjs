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
  ["receivers", "snapshot", "info", "journal", "history", "adev", "status"].map((n) => [n, fixture(n)]),
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
export function receiver(serial, mode, present) {
  return { serial, up: true, mode, present, delay: {} };
}

// The state a test drives.  `units` is the list the server reports.
export class Fake {
  constructor(units) {
    this.units = units;
    // Requests seen, by endpoint, for counting.
    this.seen = {};
  }

  unit(serial) {
    return this.units.find((u) => u.serial === serial) ?? this.units[0];
  }

  answer(endpoint, u) {
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
      const wait = endpoint === "receivers" ? 0 : (u.delay[endpoint] ?? 0);
      if (wait) await new Promise((r) => setTimeout(r, wait));
      // The page may have gone on without this answer.
      await route
        .fulfill({ contentType: "application/json", body: JSON.stringify(this.answer(endpoint, u)) })
        .catch(() => {});
    });
  }
}
