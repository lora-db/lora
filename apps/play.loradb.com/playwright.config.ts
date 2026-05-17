/**
 * Playwright config for the LoraDB playground.
 *
 * Boots a production Next.js server (`next start`) on port 4321 and runs the
 * smoke suite against it in a real Chromium. The app is an
 * IndexedDB-backed WASM playground, so `fullyParallel` is off — every test
 * shares the same `loradb-play` database and resets it explicitly in the
 * helpers.
 */

import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./tests/e2e",
  timeout: 60_000,
  expect: {
    timeout: 10_000,
  },
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: 1,
  reporter: [["list"]],
  use: {
    baseURL: "http://localhost:4321",
    trace: "retain-on-failure",
    headless: true,
    viewport: { width: 1440, height: 900 },
    locale: "en-US",
    acceptDownloads: true,
    testIdAttribute: "data-testid",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
  webServer: {
    command: "yarn next start -p 4321",
    port: 4321,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
    stdout: "pipe",
    stderr: "pipe",
  },
});
