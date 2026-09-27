// Browser tests for smartclock-web's pages.
//
// The pages come from the real server, built by cargo; its API is faked
// per test (fake.mjs), so the server is started with empty directories
// and never reaches a daemon or a log.  Playwright's own Chromium is in
// node_modules (PLAYWRIGHT_BROWSERS_PATH=0), not the home directory.

import { defineConfig } from "@playwright/test";

const PORT = 9982;

export default defineConfig({
  testDir: ".",
  testMatch: "*.spec.mjs",
  // The tests wait on the pages' own timers, a second for the strip and
  // ten for the receiver list, so they run side by side.
  fullyParallel: true,
  timeout: 60000,
  use: { baseURL: `http://127.0.0.1:${PORT}` },
  webServer: {
    command:
      `sh -c 'd=$(mktemp -d) && exec ../../../../target/debug/smartclock-web ` +
      `--listen 127.0.0.1:${PORT} --run-dir "$d" --log-dir "$d"'`,
    url: `http://127.0.0.1:${PORT}/`,
    reuseExistingServer: false,
  },
});
