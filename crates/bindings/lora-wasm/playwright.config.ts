/**
 * Playwright runs a single browser smoke test that loads
 * `examples/browser.html` through Vite and asserts a full query round-trip
 * on the worker-backed Database. This validates the non-blocking browser
 * path end-to-end (WASM inside a Web Worker).
 */

import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./test-browser",
  timeout: 60_000,
  fullyParallel: false,
  reporter: [["list"]],
  use: {
    baseURL: "http://localhost:5180",
    trace: "off",
    headless: true,
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
  webServer: {
    command: "npx vite --config vite.config.ts",
    url: "http://localhost:5180/examples/browser.html",
    reuseExistingServer: false,
    timeout: 60_000,
    stdout: "pipe",
    stderr: "pipe",
  },
});
