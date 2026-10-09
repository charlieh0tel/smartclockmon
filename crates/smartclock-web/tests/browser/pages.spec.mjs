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

// The range in the address: `last`, or the fixed pair.
function shown(page) {
  const q = new URL(page.url()).searchParams;
  return { last: q.get("last"), from: Number(q.get("from")), to: Number(q.get("to")) };
}

// Open `path` with `query` in the address, once its control is drawn.
async function openAt(page, path, query) {
  await twoUnits().install(page);
  await page.goto(`${path}?receiver=${B}&${query}`);
  await expect(page.locator("#range-back")).toBeVisible();
  await page.waitForFunction("page !== null && unit !== null");
}

// A fixed hour, a day ago, which no step or zoom below brings near now.
const DAY_AGO = Math.round(Date.now() / 1000) - 86400;
const FIXED_HOUR = `from=${DAY_AGO}&to=${DAY_AGO + 3600}`;

for (const path of ["/", "/compare"]) {
  test(`a drag on the ${path} charts fixes the range, and a double click zooms out`, async ({ page }) => {
    await openAt(page, path, "last=21600");
    await page.waitForFunction(() => charts.length > 1);
    await dragAcross(page);
    await expect.poll(() => shown(page).last).toBeNull();
    const dragged = shown(page);
    expect(dragged.to - dragged.from).toBeGreaterThan(0);
    await page.waitForFunction(() => charts.length > 1);
    await page.evaluate(() => charts[0].over.dispatchEvent(new MouseEvent("dblclick")));
    // The recorded history is long past, so the zoomed window stays fixed.
    await expect.poll(() => shown(page).to - shown(page).from).toBeGreaterThan(dragged.to - dragged.from + 1);
    const out = shown(page);
    expect(Math.abs(out.to - out.from - 2 * (dragged.to - dragged.from))).toBeLessThanOrEqual(2);
    expect(Math.abs(out.from + out.to - dragged.from - dragged.to)).toBeLessThanOrEqual(2);
  });
}

test("‹ and › move a fixed range by half, − doubles it, and Back undoes each", async ({ page }) => {
  await openAt(page, "/", FIXED_HOUR);
  await page.locator("#range-back").click();
  await expect.poll(() => shown(page).from).toBe(DAY_AGO - 1800);
  await page.locator("#range-forward").click();
  await expect.poll(() => shown(page).from).toBe(DAY_AGO);
  await page.locator("#range-out").click();
  await expect.poll(() => shown(page)).toEqual({ last: null, from: DAY_AGO - 1800, to: DAY_AGO + 5400 });
  await page.goBack();
  await expect.poll(() => shown(page).from).toBe(DAY_AGO);
  await expect(page.locator(".u-over").first()).toBeVisible();
  await page.goBack();
  await expect.poll(() => shown(page).from).toBe(DAY_AGO - 1800);
  await page.goForward();
  await expect.poll(() => shown(page).from).toBe(DAY_AGO);
  // The control is drawn for the range returned to.
  await expect(page.locator("#ranges")).toContainText(
    await page.evaluate((t) => shortTime(t), DAY_AGO),
  );
});

test("a moving range steps back to a fixed one, and forward to moving again", async ({ page }) => {
  await openAt(page, "/", "last=3600");
  await expect(page.locator("#range-forward")).toBeDisabled();
  await page.locator("#range-back").click();
  await expect.poll(() => shown(page).last).toBeNull();
  const back = shown(page);
  expect(back.to - back.from).toBe(3600);
  expect(Math.abs(back.to - (Date.now() / 1000 - 1800))).toBeLessThan(60);
  await expect(page.locator("#range-refresh")).toBeDisabled();
  await page.locator("#range-forward").click();
  await expect.poll(() => shown(page).last).toBe("3600");
  await page.locator("#range-out").click();
  await expect.poll(() => shown(page).last).toBe("7200");
});

test("the range buttons stay where they are as the range changes", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await openAt(page, "/", "last=3600");
  const places = () =>
    page.locator("#ranges button").evaluateAll((bs) =>
      bs.map((b) => `${b.id || b.textContent}@${Math.round(b.getBoundingClientRect().x)}`));
  const live = await places();
  // One height and one top for every control, so their text lines up.
  const boxes = await page.locator("#ranges button, #ranges select, #ranges input").evaluateAll((es) =>
    es.map((e) => { const r = e.getBoundingClientRect(); return `${Math.round(r.top)}+${Math.round(r.height)}`; }));
  expect(new Set(boxes).size, boxes.join(" ")).toBe(1);
  await page.locator("#range-back").click();
  await expect.poll(() => shown(page).last).toBeNull();
  expect(await places()).toEqual(live);
  await page.locator("#range-forward").click();
  await expect.poll(() => shown(page).last).toBe("3600");
  expect(await places()).toEqual(live);
});

test("Grafana's time keys move and zoom the range", async ({ page }) => {
  await openAt(page, "/", FIXED_HOUR);
  const chord = async (key) => {
    await page.keyboard.press("t");
    await page.keyboard.press(key);
  };
  await chord("ArrowLeft");
  await expect.poll(() => shown(page).from).toBe(DAY_AGO - 1800);
  await chord("ArrowRight");
  await expect.poll(() => shown(page).from).toBe(DAY_AGO);
  await chord("-");
  await expect.poll(() => shown(page).from).toBe(DAY_AGO - 1800);
  // As typed: Shift, then the key, which must not end the chord.
  await chord("Shift+Equal");
  await expect.poll(() => shown(page).from).toBe(DAY_AGO);
  await page.keyboard.press("Control+z");
  await expect.poll(() => shown(page).from).toBe(DAY_AGO - 1800);
  // An arrow without the `t` first, or typed into a control, is not a
  // step.
  await page.keyboard.press("ArrowLeft");
  await page.locator("#columns input").first().focus();
  await chord("ArrowLeft");
  await page.waitForTimeout(300);
  expect(shown(page).from).toBe(DAY_AGO - 1800);
  await page.locator("#columns input").first().blur();
  await page.locator('#ranges button[data-last="21600"]').click();
  await expect.poll(() => shown(page).last).toBe("21600");
  await chord("a");
  await expect.poll(() => shown(page).last).toBeNull();
  expect(shown(page).to - shown(page).from).toBe(21600);
});

test("Grafana's forms of the range are read and rewritten", async ({ page }) => {
  await openAt(page, "/", "from=now-6h&to=now");
  expect(shown(page)).toEqual({ last: "21600", from: 0, to: 0 });
  await page.goto(`/?receiver=${B}&from=${DAY_AGO * 1000}&to=${(DAY_AGO + 3600) * 1000}`);
  await expect.poll(() => shown(page)).toEqual({ last: null, from: DAY_AGO, to: DAY_AGO + 3600 });
  await page.goto(`/?receiver=${B}&from=${new Date(DAY_AGO * 1000).toISOString()}&to=now-1h`);
  await expect.poll(() => shown(page).from).toBe(DAY_AGO);  // A fixed start up to now still moves with the clock, as in Grafana.
  await page.goto(`/?receiver=${B}&from=${DAY_AGO * 1000}&to=now`);
  await expect.poll(() => shown(page).last).not.toBeNull();
  expect(Math.abs(Number(shown(page).last) - 86400)).toBeLessThan(60);
});

test("the satellite counts share a chart, each named in its own color", async ({ page }) => {
  const fake = twoUnits();
  fake.everyColumn = true;
  await fake.install(page);
  await page.goto(`/?receiver=${B}&last=3600`);
  await page.waitForFunction(() => charts.some((c) => c.smartclockLines.length === 2));
  const pair = await page.evaluate(() => {
    const chart = charts.find((c) => c.smartclockLines.length === 2);
    const heading = chart.root.querySelector(".u-title");
    return {
      columns: chart.smartclockLines.map((l) => l.column),
      colored: [...heading.querySelectorAll("span[style]")].map((s) => s.textContent),
    };
  });
  expect(pair.columns).toEqual(["tracking", "not_tracking"]);
  expect(pair.colored).toEqual(["tracked", "not tracked"]);
  // A cell apiece in the readout, colored as the lines are.
  const keys = await page.locator("#readout .k[style]").allTextContents();
  expect(keys).toEqual(["# sats tracked", "# sats not tracked"]);
});

// A fake with a room thermometer and hygrometer, and the receiver's
// temperature chart to sit them under.
function withSensors() {
  const fake = twoUnits();
  fake.everyColumn = true;
  fake.sensors = [
    { name: "room", quantity: "temperature", unit: "C", value: 21.5 },
    { name: "bench", quantity: "temperature", unit: "C", value: 23 },
    { name: "room", quantity: "humidity", unit: "%RH", value: 41 },
  ];
  return fake;
}

test("the host's sensors are charted below the receiver's temperature, on its time axis", async ({ page }) => {
  await withSensors().install(page);
  await page.goto(`/?receiver=${B}&last=3600`);
  await page.waitForFunction(() => charts.some((c) => c.smartclockLines[0].column.startsWith("sensor:")));
  const stack = await page.evaluate(() => charts.map((c) => c.smartclockLines[0].column));
  const at = stack.indexOf("temperature_c");
  expect(stack.slice(at, at + 3)).toEqual(["temperature_c", "sensor:temperature", "sensor:humidity"]);
  // One time axis for the whole stack.
  const axes = await page.evaluate(() => new Set(charts.map((c) => c.data[0].join(","))).size);
  expect(axes).toBe(1);
  const heading = await page.evaluate(() => {
    const chart = charts.find((c) => c.smartclockLines[0].column === "sensor:temperature");
    const title = chart.root.querySelector(".u-title");
    return { text: title.textContent, hover: title.title };
  });
  expect(heading.text).toBe("Sensors, temperature, C: bench, room");
  expect(heading.hover).toContain("/sys/fake/room");
});

test("with sensors joined in, the readout has a value for every line wherever the cursor is", async ({ page }) => {
  await withSensors().install(page);
  await page.goto(`/?receiver=${B}&last=3600`);
  await page.waitForFunction(() => charts.some((c) => c.smartclockLines[0].column === "sensor:temperature"));
  const box = await page.evaluate(() => charts[0].over.getBoundingClientRect().toJSON());
  for (const at of [0.2, 0.3, 0.31, 0.32]) {
    await page.mouse.move(box.x + box.width * at, box.y + box.height / 2);
    const cells = await page.locator("#readout .v").allTextContents();
    // The time, then a value per line: none dashed.
    expect(cells.slice(1).filter((v) => v === "--"), cells.join(" ")).toEqual([]);
  }
});

test("a sensor keeps its color on every chart", async ({ page }) => {
  await withSensors().install(page);
  await page.goto(`/?receiver=${B}&last=3600`);
  await page.waitForFunction(() => charts.some((c) => c.smartclockLines[0].column === "sensor:humidity"));
  const colors = await page.evaluate(() =>
    charts
      .flatMap((c) => c.smartclockLines)
      .filter((l) => l.name === "room")
      .map((l) => l.color),
  );
  expect(colors.length).toBe(2);
  expect(colors[0]).toBe(colors[1]);
});

test("the sensors can be left out, and stay out across a reload", async ({ page }) => {
  await withSensors().install(page);
  await page.goto(`/?receiver=${B}&last=3600`);
  await page.locator("#columns input[data-sensors]").uncheck();
  await expect.poll(() => new URL(page.url()).searchParams.get("sensors")).toBe("off");
  await page.waitForFunction(() => charts.length && !charts.some((c) => c.smartclockLines[0].column.startsWith("sensor:")));
  await page.reload();
  await expect(page.locator("#columns input[data-sensors]")).not.toBeChecked();
});

for (const path of ["/", "/status"]) {
  test(`the strip on ${path} shows each sensor's current reading`, async ({ page }) => {
    await withSensors().install(page);
    await page.goto(`${path}?receiver=${B}`);
    await expect(page.locator("#status .sensor")).toHaveText([
      "21.5 Croom temp",
      "23.0 Cbench temp",
      "41 %RHroom humidity",
    ]);
  });
}

test("the compare page charts the sensors below the receivers, on their time axis", async ({ page }) => {
  await withSensors().install(page);
  await page.goto(`/compare?receiver=${B}&last=86400`);
  await page.waitForFunction(() => charts.some((c) => c.root.querySelector(".u-title").textContent.startsWith("Sensors")));
  const titles = await page.evaluate(() => charts.map((c) => c.root.querySelector(".u-title").textContent));
  const first = titles.findIndex((t) => t.startsWith("Sensors"));
  expect(titles.slice(first)).toEqual(["Sensors, temperature, C", "Sensors, relative humidity, %"]);
  expect(await page.evaluate(() => new Set(charts.map((c) => c.data[0].join(","))).size)).toBe(1);
});

test("with no receiver logged, the sensors are charted alone", async ({ page }) => {
  const fake = new Fake([]);
  fake.sensors = [{ name: "room", quantity: "temperature", unit: "C", value: 21.5 }];
  await fake.install(page);
  await page.goto("/?last=3600");
  await page.waitForFunction(() => charts.length === 1);
  expect(await page.evaluate(() => charts[0].smartclockLines[0].column)).toBe("sensor:temperature");
});

for (const path of ["/adev", "/compare", "/correlation"]) {
  test(`the ${path} page fits a phone's width`, async ({ page }) => {
    await page.setViewportSize({ width: 360, height: 800 });
    const fake = twoUnits();
    // Correlation needs both of its measures, and its whole log, which
    // the recording's times are inside.
    fake.everyColumn = true;
    await fake.install(page);
    await page.goto(`${path}?receiver=${B}&last=` + (path === "/correlation" ? "all" : "86400"));
    await page.waitForFunction(() => document.querySelectorAll(".uplot").length > 0);
    await page.waitForTimeout(300);
    const [scroll, client] = await page.evaluate(() => [
      document.documentElement.scrollWidth,
      document.documentElement.clientWidth,
    ]);
    expect(scroll).toBeLessThanOrEqual(client);
  });
}

for (const path of ["/", "/compare"]) {
  test(`a redraw of ${path} keeps the page where it was scrolled to`, async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 600 });
    await withSensors().install(page);
    await page.goto(`${path}?receiver=${B}&last=3600`);
    await page.waitForFunction(() => charts.length >= 3);
    await page.waitForTimeout(1500);
    await page.mouse.move(5, 5);
    await page.evaluate(() => window.scrollTo(0, 600));
    // What the refresh timer and the reload button run.
    const done = await page.evaluate(() => new Promise((resolve) => {
      renew(true);
      setTimeout(() => resolve(Math.round(window.scrollY)), 1500);
    }));
    expect(done).toBe(600);
  });
}

test("the compare page keeps its charts up while it reads them again", async ({ page }) => {
  const fake = withSensors();
  // A stability measurement takes time; the old curves stay meanwhile.
  for (const unit of fake.units) unit.delay = { adev: 1000, notes: 300 };
  await fake.install(page);
  await page.goto(`/compare?receiver=${B}&last=3600`);
  await page.waitForFunction(() => charts.length >= 3 && deviationChart);
  const empty = await page.evaluate(() => new Promise((resolve) => {
    let blank = 0;
    const until = performance.now() + 2500;
    const frame = () => {
      if (!$("charts").querySelector(".uplot") || !$("chart-deviation").querySelector(".uplot")) blank++;
      if (performance.now() < until) requestAnimationFrame(frame);
      else resolve(blank);
    };
    renew(true);
    requestAnimationFrame(frame);
  }));
  expect(empty).toBe(0);
});

test("a host without sensors offers none", async ({ page }) => {
  await openAt(page, "/", "last=3600");
  await page.waitForFunction(() => charts.length > 0);
  await expect(page.locator("#columns input[data-sensors]")).toHaveCount(0);
  await expect(page.locator("#status .stat").first()).toBeVisible();
  await expect(page.locator("#status .sensor")).toHaveCount(0);
});

test("every series has hover text, on the history and the compare page", async ({ page }) => {
  await openAt(page, "/", "last=3600");
  const labels = await page.locator("#columns label").evaluateAll((ls) => ls.map((l) => [l.textContent, l.title]));
  expect(labels.filter(([, title]) => !title)).toEqual([]);
  await page.waitForFunction(() => charts.length > 1);
  const titles = await page.locator(".u-title").evaluateAll((ts) => ts.map((t) => [t.textContent, t.title]));
  expect(titles.filter(([, title]) => !title)).toEqual([]);
  await page.goto(`/compare?receiver=${B}&last=3600`);
  await page.waitForFunction(() => charts.length > 1);
  const compared = await page.locator("#charts .u-title").evaluateAll((ts) => ts.map((t) => [t.textContent, t.title]));
  expect(compared.filter(([, title]) => !title)).toEqual([]);
  // The hover area is the title's words, not the chart's width.
  const widths = await page.locator("#charts .u-title").evaluateAll((ts) =>
    ts.map((t) => [t.getBoundingClientRect().width, t.parentElement.getBoundingClientRect().width]),
  );
  expect(widths.filter(([title, chart]) => title > chart / 2)).toEqual([]);
});

test("the columns chosen ride in the address", async ({ page }) => {
  await openAt(page, "/", "last=3600");
  await page.locator('#columns input[data-col="efc_percent"]').uncheck();
  await expect.poll(() => new URL(page.url()).searchParams.get("columns")).not.toBeNull();
  expect(new URL(page.url()).searchParams.get("columns")).not.toContain("efc_percent");
});

test("Back keeps the receiver chosen since", async ({ page }) => {
  await openAt(page, "/", "last=3600");
  await page.locator('#ranges button[data-last="21600"]').click();
  await expect.poll(() => shown(page).last).toBe("21600");
  await page.locator("#unit").selectOption(A);
  await expect.poll(() => new URL(page.url()).searchParams.get("receiver")).toBe(A);
  await page.goBack();
  await expect.poll(() => shown(page).last).toBe("3600");
  await expect.poll(() => new URL(page.url()).searchParams.get("receiver")).toBe(A);
});

test("a moving range is read again on the refresh chosen, and not when off", async ({ page }) => {
  const fake = twoUnits();
  await fake.install(page);
  await page.goto(`/?receiver=${B}&last=3600&refresh=5s`);
  await expect.poll(() => fake.seen.history ?? 0).toBeGreaterThan(1);
  const before = fake.seen.history;
  await expect.poll(() => fake.seen.history, { timeout: 8000 }).toBeGreaterThan(before);
  await page.locator("#range-refresh").selectOption("off");
  expect(new URL(page.url()).searchParams.get("refresh")).toBe("off");
  const stopped = fake.seen.history;
  await page.waitForTimeout(6000);
  expect(fake.seen.history).toBe(stopped);
});

test("the stability page offers no refresh faster than a minute, and shows one asked for as a minute", async ({ page }) => {
  await openAt(page, "/adev", "last=3600&refresh=5s");
  const offered = await page.locator("#range-refresh option").evaluateAll((o) => o.map((x) => x.value));
  expect(offered.slice(0, 3)).toEqual(["off", "auto", "1m"]);
  await expect(page.locator("#range-refresh")).toHaveValue("1m");
  expect(new URL(page.url()).searchParams.get("refresh")).toBe("1m");
});

test("a fixed range that reaches past now is read again; one wholly past is not", async ({ page }) => {
  const now = Math.round(Date.now() / 1000);
  await openAt(page, "/", `from=${now - 600}&to=${now + 3000}&refresh=5s`);
  await expect(page.locator("#range-refresh")).toBeEnabled();
  expect(await page.evaluate("refreshEvery()")).toBe(5);
  await page.goto(`/?receiver=${B}&${FIXED_HOUR}&refresh=5s`);
  await expect(page.locator("#range-refresh")).toBeDisabled();
  expect(await page.evaluate("refreshEvery()")).toBeNull();
});

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

test("the correlation page sets two measures against each other, over locked readings unless told not to", async ({ page }) => {
  const fake = twoUnits();
  fake.everyColumn = true;
  const asked = [];
  page.on("request", (r) => {
    if (r.url().includes("/api/history") && r.url().includes("from=")) asked.push(new URL(r.url()).searchParams);
  });
  await fake.install(page);
  // The whole log, which the recording's times are inside.
  await page.goto(`/correlation?receiver=${A}&last=all`);
  await page.waitForFunction(() => scatterChart !== null && lagChart !== null);
  await expect(page.locator("#numbers")).toContainText("r of changes");
  expect(await page.locator("#pick-y").inputValue()).toBe("efc_percent");
  expect(await page.locator("#pick-x").inputValue()).toBe("temperature_c");
  expect(asked.length).toBeGreaterThan(0);
  expect(asked.every((q) => q.get("locked") === "1")).toBe(true);
  // The other receiver's EFC is offered too.
  await expect(page.locator(`#pick-x option[value="efc:${B}"]`)).toHaveCount(1);
  asked.length = 0;
  await page.locator("#locked").uncheck();
  await expect.poll(() => asked.length).toBeGreaterThan(0);
  expect(asked.some((q) => q.has("locked"))).toBe(false);
  expect(new URL(page.url()).searchParams.get("locked")).toBe("0");
});

test("a length typed over a fixed range makes it the moving range of that length", async ({ page }) => {
  await twoUnits().install(page);
  await page.goto(`/?receiver=${A}&from=1790539803&to=1790554203`);
  const box = page.locator("#range-n");
  await expect(box).toBeEnabled();
  await expect(box).toHaveValue("4");
  await box.fill("6");
  await box.press("Enter");
  await expect.poll(() => new URL(page.url()).searchParams.get("last")).toBe("21600");
  expect(new URL(page.url()).searchParams.has("from")).toBe(false);
});

test("a sensor read less often than the buckets are wide is still drawn as a line", async ({ page }) => {
  // The fake's sensors are read every fourth recorded time, so most
  // buckets are empty; the line must run across them, not vanish.
  await withSensors().install(page);
  await page.goto(`/correlation?receiver=${B}&last=all&x=sensor:temperature:room`);
  await page.waitForFunction(() => timeCharts.length === 2);
  const drawn = await page.evaluate(() => {
    const ys = timeCharts[1].data[1];
    return { points: ys.length, missing: ys.filter((v) => v == null).length };
  });
  expect(drawn.points).toBeGreaterThan(1);
  expect(drawn.missing).toBe(0);
});

test("a note is added, edited and deleted from the journal, through the receiver's daemon", async ({ page }) => {
  const fake = twoUnits();
  await open(page, fake, "/", B);
  await page.locator('#journal-tabs button[data-stream="notes"]').click();
  await expect(page.locator("#note-form")).toBeVisible();

  await page.locator("#note-text").fill("moved the antenna");
  await page.locator("#note-add").click();
  await expect(page.locator("#note-said")).toHaveText("added");
  expect(fake.writes[0]).toEqual({
    action: "add",
    body: { receiver: B, text: "moved the antenna" },
    type: "application/json",
  });

  await page.locator(".note-edit").first().click();
  await page.locator(".note-new-text").fill("moved the antenna 2 m");
  await page.locator(".note-save").click();
  await expect(page.locator("#note-said")).toHaveText("changed");
  const edited = fake.writes[1];
  expect(edited.action).toBe("edit");
  expect(edited.body).toMatchObject({ receiver: B, id: 7, text: "moved the antenna 2 m", was: `note of ${B}` });

  // One click arms the delete; only the second sends it.
  await page.locator(".note-delete").first().click();
  expect(fake.writes.length).toBe(2);
  await page.locator(".note-delete").first().click();
  await expect(page.locator("#note-said")).toHaveText("deleted");
  expect(fake.writes[2]).toMatchObject({ action: "delete", body: { receiver: B, id: 7, was: `note of ${B}` } });
});

test("a note the daemon refuses says why", async ({ page }) => {
  const fake = twoUnits();
  fake.writeAnswer = { error: "note 7 has changed since it was read; read it again" };
  await open(page, fake, "/", B);
  await page.locator('#journal-tabs button[data-stream="notes"]').click();
  await page.locator(".note-edit").first().click();
  await page.locator(".note-new-text").fill("something else");
  await page.locator(".note-save").click();
  await expect(page.locator("#note-said")).toContainText("has changed since it was read");
});


test("a note being edited is not drawn over by the page refreshing", async ({ page }) => {
  const fake = twoUnits();
  await fake.install(page);
  await page.goto(`/?receiver=${B}&refresh=5s`);
  await page.locator('#journal-tabs button[data-stream="notes"]').click();
  await page.locator(".note-edit").first().click();
  await page.locator(".note-new-text").fill("half typed");
  const reads = fake.seen.journal;
  await page.waitForTimeout(7000);
  await expect(page.locator(".note-new-text")).toHaveValue("half typed");
  expect(fake.seen.journal).toBe(reads);
});

test("a note's time is now unless set, in the zone shown, and never in the future", async ({ page }) => {
  const fake = twoUnits();
  await open(page, fake, "/", B);
  await page.locator('#journal-tabs button[data-stream="notes"]').click();
  await expect(page.locator("#note-now")).toContainText("when: now");
  await page.locator("#note-earlier").click();
  await expect(page.locator("#note-then .note-zone")).not.toBeEmpty();

  await page.locator("#note-text").fill("tomorrow");
  await page.locator("#note-when").fill(
    await page.evaluate(() => toField(Date.now() + 86400000)),
  );
  await page.locator("#note-add").click();
  await expect(page.locator("#note-said")).toContainText("in the future");
  expect(fake.writes.length).toBe(0);

  // Edited text only: the time is left as it was, not rounded to the
  // minute the field shows.
  await page.locator(".note-edit").first().click();
  await page.locator(".note-new-text").fill("text only");
  await page.locator(".note-save").click();
  await expect(page.locator("#note-said")).toHaveText("changed");
  expect(fake.writes[0].body.at).toBeUndefined();
});

test("a note's time can be picked by clicking a chart", async ({ page }) => {
  const fake = twoUnits();
  await open(page, fake, "/", B);
  await page.locator('#journal-tabs button[data-stream="notes"]').click();
  await page.waitForFunction(() => charts.length > 0);
  await page.locator("#note-earlier").click();
  await page.locator("#note-pick").click();
  const { x, y, expected } = await page.evaluate(() => {
    const chart = charts[0];
    const box = chart.over.getBoundingClientRect();
    const left = box.width / 2;
    return {
      x: box.left + left,
      y: box.top + box.height / 2,
      expected: toField(chart.posToVal(left, "x") * 1000),
    };
  });
  await page.mouse.click(x, y);
  await expect(page.locator("#note-when")).toHaveValue(expected);
  expect(await page.evaluate(() => document.body.dataset.picking)).toBeUndefined();
});

test.describe("in Los Angeles", () => {
  test.use({ timezoneId: "America/Los_Angeles" });

  test("times can be shown and entered in UTC, and the choice is kept in the address", async ({ page }) => {
    await open(page, twoUnits(), "/", B);
    await page.locator('#journal-tabs button[data-stream="notes"]').click();
    await expect(page.locator("#journal td").first()).toContainText(/P[DS]T/);
    // Round from local 12-hour, through local 24-hour, to UTC.
    await expect(page.locator("#range-zone")).toHaveText(/^P[DS]T 12h$/);
    await page.locator("#range-zone").click();
    await expect(page.locator("#range-zone")).toHaveText(/^P[DS]T 24h$/);
    await expect.poll(() => new URL(page.url()).searchParams.get("tz")).toBe("local24");
    expect(await page.evaluate(() => showClock(Date.parse("2026-10-08T16:35:00Z")))).toBe("09:35:00");
    await page.locator("#range-zone").click();
    await expect(page.locator("#range-zone")).toHaveText("UTC");
    await expect.poll(() => new URL(page.url()).searchParams.get("tz")).toBe("utc");
    await expect(page.locator("#journal td").first()).toContainText("UTC");
    expect(await page.evaluate(() => fromField("2026-10-08T16:35"))).toBe("2026-10-08T16:35:00.000Z");
    // UTC on a 24-hour clock, on the axes as elsewhere.
    expect(await page.evaluate(() => showClock(Date.parse("2026-10-08T16:35:00Z")))).toBe("16:35:00");
    await page.waitForFunction(() => charts.length > 0 && charts.at(-1).axes[0]._values);
    const ticks = await page.evaluate(() => charts.at(-1).axes[0]._values.join(" "));
    expect(ticks).not.toMatch(/[ap]m/i);
    expect(await page.evaluate(() => toField(Date.parse("2026-10-08T16:35:00Z")))).toBe("2026-10-08T16:35");
    for (const a of await page.locator("nav a").all()) {
      expect(new URL(await a.getAttribute("href"), page.url()).searchParams.get("tz")).toBe("utc");
    }
    await page.locator("#range-zone").click();
    await expect(page.locator("#range-zone")).toHaveText(/^P[DS]T 12h$/);
    await expect.poll(() => new URL(page.url()).searchParams.has("tz")).toBe(false);
  });

  test("a local time a clock change skips or doubles is refused", async ({ page }) => {
    await open(page, twoUnits(), "/", B);
    const refusal = (value) =>
      page.evaluate((v) => {
        try {
          return fromField(v);
        } catch (e) {
          return e.message;
        }
      }, value);
    expect(await refusal("2026-03-08T02:30")).toContain("skipped");
    expect(await refusal("2026-11-01T01:30")).toContain("happens twice");
    expect(await refusal("2026-10-08T09:35")).toBe("2026-10-08T16:35:00.000Z");
  });
});
