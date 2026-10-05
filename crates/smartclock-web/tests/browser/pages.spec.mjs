// Every page must behave the same way at the edges: on a change of
// receiver, when an answer arrives late for a receiver no longer
// chosen, when the daemon goes and when it comes back, when a receiver
// appears, and in the strip across all of it.  So every test here runs
// against every page.

import { expect, test } from "@playwright/test";

import { Fake, receiver } from "./fake.mjs";

const A = "1111A11111";
const B = "2222A22222";

// For each page: where it is, which endpoints its content comes from,
// and whose data it is showing -- the serial, or null when it shows
// none.
const PAGES = [
  {
    name: "live",
    path: "/",
    reads: ["history", "journal", "info"],
    daemon: false,
    whose: async (page) => {
      const about = await page.locator("#about").textContent();
      return [A, B].find((s) => about.includes(s)) ?? null;
    },
  },
  {
    name: "status",
    path: "/status",
    reads: ["status"],
    daemon: true,
    whose: async (page) => {
      const text = await page.locator("#screen").textContent();
      return text.match(/^screen of (\S+)/)?.[1] ?? null;
    },
  },
  {
    name: "stability",
    path: "/adev",
    reads: ["adev"],
    daemon: false,
    whose: async (page) => {
      const state = await page.locator("#state").textContent();
      return state.includes("111readings") ? A : state.includes("222readings") ? B : null;
    },
  },
];

// The strip names neither serial, so each unit's snapshot carries its
// own mode.
const MODE = { [A]: "Locked", [B]: "Holdover" };
const stripWhose = async (page) => {
  const text = await page.locator("#status").textContent();
  return Object.keys(MODE).find((s) => text.includes(MODE[s])) ?? null;
};

function twoUnits() {
  return new Fake([receiver(A, MODE[A], 111), receiver(B, MODE[B], 222)]);
}

async function open(page, fake, path, serial = A) {
  await fake.install(page);
  await page.goto(`${path}?receiver=${serial}`);
}

// Watch `probe` until `until` ms have passed, and return every value it
// took, so a test can say what never appeared as well as what did.
async function watch(page, probe, until) {
  const seen = new Set();
  const end = Date.now() + until;
  while (Date.now() < end) {
    seen.add(await probe(page));
    await page.waitForTimeout(100);
  }
  return seen;
}

test("a unit is shown only what it measures, and follows a switch", async ({ page }) => {
  const measured = (without) =>
    ["time_interval_s", "efc_percent", "tracking", "temperature_c", "tfom", "ffom", "not_tracking"]
      .filter((c) => c !== without);
  const fake = new Fake([
    receiver(A, MODE[A], 111, measured("temperature_c")),
    receiver(B, MODE[B], 222, measured(null)),
  ]);
  await open(page, fake, "/");
  await expect.poll(() => stripWhose(page)).toBe(A);
  await expect(page.locator("#status")).not.toContainText("internal temp");
  await expect(page.locator('#columns input[data-col="temperature_c"]')).toHaveCount(0);
  await page.locator("#unit").selectOption(B);
  await expect.poll(() => stripWhose(page)).toBe(B);
  await expect(page.locator("#status")).toContainText("internal temp");
  await expect(page.locator('#columns input[data-col="temperature_c"]')).toHaveCount(1);
});

test("a note is marked on the charts and named when the cursor is on it", async ({ page }) => {
  await open(page, twoUnits(), "/", B);
  await expect(page.locator("#journal-tabs")).toContainText("notes (1)");
  await page.waitForFunction(() => charts.length > 0 && notes.length > 0);
  const { x, y } = await page.evaluate(() => {
    const chart = charts[0];
    const box = chart.over.getBoundingClientRect();
    return { x: box.left + chart.valToPos(notes[0][0], "x"), y: box.top + box.height / 2 };
  });
  await page.mouse.move(x, y);
  await expect(page.locator("#readout")).toContainText(`note of ${B}`);
  await page.mouse.move(x + 40, y);
  await expect(page.locator("#readout")).not.toContainText("note of");
});

test("the range and the receiver go along to the next page", async ({ page }) => {
  const fake = twoUnits();
  await open(page, fake, "/", B);
  await page.locator('#ranges button[data-last="21600"]').click();
  await page.locator("nav a", { hasText: "Stability" }).click();
  await expect(page).toHaveURL(/\/adev\?/);
  const q = new URL(page.url()).searchParams;
  expect(q.get("receiver")).toBe(B);
  expect(q.get("last")).toBe("21600");
  await expect(page.locator('#ranges button[data-last="21600"]')).toHaveAttribute("aria-pressed", "true");
});

for (const p of PAGES) {
  test.describe(p.name, () => {
    test("shows the chosen receiver, in the page and the strip", async ({ page }) => {
      const fake = twoUnits();
      await open(page, fake, p.path, B);
      await expect.poll(() => p.whose(page)).toBe(B);
      await expect.poll(() => stripWhose(page)).toBe(B);
      await expect(page.locator("#unit")).toHaveValue(B);
    });

    test("links to the source and the issues, under the copyright", async ({ page }) => {
      await open(page, twoUnits(), p.path);
      const colophon = page.locator("#colophon");
      await expect(colophon).toContainText("© 2026 Christopher Hoover");
      await expect(colophon).toContainText("GPL-3.0-or-later");
      await expect(colophon.locator("a", { hasText: "source" }))
        .toHaveAttribute("href", "https://github.com/charlieh0tel/smartclockmon");
      await expect(colophon.locator("a", { hasText: "issues" }))
        .toHaveAttribute("href", "https://github.com/charlieh0tel/smartclockmon/issues");
    });

    test("a change of receiver clears at once, then shows the new one", async ({ page }) => {
      const fake = twoUnits();
      await open(page, fake, p.path);
      await expect.poll(() => p.whose(page)).toBe(A);
      await expect.poll(() => stripWhose(page)).toBe(A);
      for (const e of [...p.reads, "snapshot"]) fake.unit(B).delay[e] = 1500;
      await page.locator("#unit").selectOption(B);
      // Before B has answered, nothing of A's is left up.
      expect(await p.whose(page)).toBeNull();
      expect(await stripWhose(page)).not.toBe(A);
      await expect.poll(() => p.whose(page)).toBe(B);
      await expect.poll(() => stripWhose(page)).toBe(B);
      expect(page.url()).toContain(`receiver=${B}`);
    });

    test("a late answer for the last receiver is dropped", async ({ page }) => {
      const fake = twoUnits();
      for (const e of [...p.reads, "snapshot"]) fake.unit(A).delay[e] = 2000;
      await open(page, fake, p.path);
      await page.waitForTimeout(300);
      await page.locator("#unit").selectOption(B);
      const seen = await watch(page, p.whose, 3500);
      const strip = await watch(page, stripWhose, 500);
      expect(seen.has(A)).toBe(false);
      expect(await p.whose(page)).toBe(B);
      expect(strip.has(A)).toBe(false);
    });

    test("the daemon going and coming back", async ({ page }) => {
      const fake = twoUnits();
      await open(page, fake, p.path);
      await expect.poll(() => p.whose(page)).toBe(A);
      fake.unit(A).up = false;
      await expect(page.locator("#status")).toContainText("no daemon", { timeout: 3000 });
      if (p.daemon) {
        // Read from the daemon: what it said has ended with it.
        await expect.poll(() => p.whose(page)).toBeNull();
        await expect(page.locator("#state")).toContainText("the link is down");
      }
      // The picker says the unit is no longer live, once it next looks.
      await expect(page.locator("#unit-seen")).not.toContainText("live on", { timeout: 12000 });
      fake.unit(A).up = true;
      await expect.poll(() => stripWhose(page), { timeout: 5000 }).toBe(A);
      await expect.poll(() => p.whose(page), { timeout: 5000 }).toBe(A);
      await expect(page.locator("#unit-seen")).toContainText("live on", { timeout: 12000 });
    });

    test("a receiver that appears is offered without a reload", async ({ page }) => {
      const fake = new Fake([receiver(A, MODE[A], 111)]);
      await open(page, fake, p.path);
      await expect.poll(() => p.whose(page)).toBe(A);
      await expect(page.locator("#unit-bar")).toBeHidden();
      fake.units.push(receiver(B, MODE[B], 222));
      await expect(page.locator("#unit-bar")).toBeVisible({ timeout: 12000 });
      await expect(page.locator("#unit option")).toHaveCount(2);
      await expect(page.locator("#unit")).toHaveValue(A);
    });

    test("the strip polls once a second", async ({ page }) => {
      const fake = twoUnits();
      await open(page, fake, p.path);
      await expect.poll(() => stripWhose(page)).toBe(A);
      const before = fake.seen.snapshot;
      await page.waitForTimeout(5000);
      const polls = fake.seen.snapshot - before;
      expect(polls).toBeGreaterThanOrEqual(4);
      expect(polls).toBeLessThanOrEqual(6);
    });

    test("the strip kept for the next page is the receiver's own", async ({ page }) => {
      const fake = twoUnits();
      await open(page, fake, p.path, A);
      await expect.poll(() => stripWhose(page)).toBe(A);
      fake.unit(B).delay.snapshot = 2000;
      await page.goto(`${p.path}?receiver=${B}`);
      const strip = await watch(page, stripWhose, 1500);
      expect(strip.has(A)).toBe(false);
      await expect.poll(() => stripWhose(page)).toBe(B);
    });
  });
}
