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
    return { x: box.left + chart.valToPos(notes[0].at, "x"), y: box.top + box.height / 2 };
  });
  await page.mouse.move(x, y);
  await expect(page.locator("#readout")).toContainText(`note of ${B}`);
  await expect(page.locator("#note-tip")).toBeVisible();
  await expect(page.locator("#note-tip")).toHaveText(`note of ${B}`);
  await page.mouse.move(x + 40, y);
  await expect(page.locator("#readout")).not.toContainText("note of");
  await expect(page.locator("#note-tip")).toBeHidden();
});

for (const path of ["/", "/compare"]) {
  test(`every chart in the ${path} stack plots the same time at the same x`, async ({ page }) => {
    // Short enough that a scrollbar appears partway down the stack as
    // charts are added (the config shows scrollbars).
    await page.setViewportSize({ width: 1200, height: 700 });
    await open(page, twoUnits(), path, B);
    await page.waitForFunction(() => charts.length > 1);
    const edges = await page.evaluate(() =>
      charts.map((c) => {
        const box = c.over.getBoundingClientRect();
        return [Math.round(box.left), Math.round(box.width)];
      }),
    );
    expect(new Set(edges.map(String)).size, JSON.stringify(edges)).toBe(1);
  });
}

// Drag across the middle half of the first chart.
async function dragAcross(page) {
  const box = await page.evaluate(() => charts[0].over.getBoundingClientRect().toJSON());
  const y = box.y + box.height / 2;
  await page.mouse.move(box.x + box.width / 4, y);
  await page.mouse.down();
  await page.mouse.move(box.x + (box.width * 3) / 4, y, { steps: 5 });
  await page.mouse.up();
}

for (const path of ["/", "/compare"]) {
  test(`a drag on the ${path} charts fixes the range, and a double click moves it again`, async ({ page }) => {
    await twoUnits().install(page);
    await page.goto(`${path}?receiver=${B}&last=21600`);
    await page.waitForFunction(() => charts.length > 1);
    await dragAcross(page);
    await expect.poll(() => new URL(page.url()).searchParams.get("from")).not.toBeNull();
    const q = new URL(page.url()).searchParams;
    const length = Number(q.get("to")) - Number(q.get("from"));
    expect(q.get("last")).toBeNull();
    expect(length).toBeGreaterThan(0);
    expect(length).toBeLessThan(21600);
    await expect(page.locator("#range-now")).toBeVisible();
    await page.waitForFunction(() => charts.length > 1);
    await page.evaluate(() => charts[0].over.dispatchEvent(new MouseEvent("dblclick")));
    await expect.poll(() => new URL(page.url()).searchParams.get("last")).not.toBeNull();
    expect(Math.abs(Number(new URL(page.url()).searchParams.get("last")) - length)).toBeLessThanOrEqual(1);
    expect(new URL(page.url()).searchParams.get("from")).toBeNull();
  });
}

test("a note clicked in the journal shows an hour either side of it", async ({ page }) => {
  await open(page, twoUnits(), "/", B);
  await page.locator('#journal-tabs button[data-stream="notes"]').click();
  const link = page.locator("#journal .note-link");
  await expect(link).toHaveText(`note of ${B}`);
  const at = Number(await link.getAttribute("data-at"));
  await link.click();
  await expect.poll(() => new URL(page.url()).searchParams.get("from")).toBe(String(Math.round(at - 3600)));
  expect(new URL(page.url()).searchParams.get("to")).toBe(String(Math.round(at + 3600)));
});

test("the compare page reads every receiver at one instant", async ({ page }) => {
  // Bucket times that differ between receivers left the legend blank
  // for every one but the receiver whose point was under the cursor.
  const fake = twoUnits();
  fake.unit(B).skew = 7;
  await open(page, fake, "/compare");
  await page.waitForFunction(() => charts.length === 2);
  // Hovered afresh on each look: the page may draw its charts again
  // after the first read, and a new chart sees the cursor only when it
  // moves.
  const legend = async () => {
    const box = await page.locator("#charts .u-over").first().boundingBox();
    await page.mouse.move(box.x + box.width / 2 + 1, box.y + box.height / 2);
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    const values = page.locator("#charts .uplot").first().locator(".u-legend .u-value");
    return [await values.nth(1).textContent(), await values.nth(2).textContent()];
  };
  await expect.poll(async () => (await legend()).every((v) => /\d/.test(v))).toBe(true);
});

test("the compare page offers a receiver that appears, and keeps its charts when a daemon goes", async ({ page }) => {
  const fake = new Fake([receiver(A, MODE[A], 111)]);
  await open(page, fake, "/compare");
  await page.waitForFunction(() => charts.length === 2);
  fake.units.push(receiver(B, MODE[B], 222));
  await expect(page.locator(`#compared input[data-serial="${B}"]`)).toBeVisible({ timeout: 12000 });
  await expect(page.locator(`#compared input[data-serial="${B}"]`)).not.toBeChecked();
  // Drawn from the logs, which a daemon going does not take away.
  fake.unit(A).up = false;
  await page.waitForTimeout(2500);
  expect(await page.evaluate(() => charts.length)).toBe(2);
});

test("the compare page marks each receiver's notes over its whole range", async ({ page }) => {
  await open(page, twoUnits(), "/compare");
  await page.waitForFunction(() => charts.length === 2 && marks.length === 2);
  const texts = await page.evaluate(() => marks.map((m) => m.text));
  expect(texts.map((t) => (t.includes(`note of ${A}`) ? A : t.includes(`note of ${B}`) ? B : t)).sort())
    .toEqual([A, B]);
});

test("the compare page overlays every receiver with readings, and drops one unticked", async ({ page }) => {
  await open(page, twoUnits(), "/compare");
  await page.waitForFunction(() => charts.length === 2 && deviationChart !== null);
  expect(await page.evaluate(() => charts[0].series.length)).toBe(3);
  await expect(page.locator(`#compared input[data-serial="${A}"]`)).toBeChecked();
  await expect(page.locator(`#compared input[data-serial="${B}"]`)).toBeChecked();
  await page.locator(`#compared input[data-serial="${B}"]`).uncheck();
  await expect.poll(() => new URL(page.url()).searchParams.get("receivers")).toBe(A);
  await page.waitForFunction(() => charts.length === 2 && charts[0].series.length === 2);
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
