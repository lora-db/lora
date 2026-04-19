/**
 * End-to-end browser smoke test.
 *
 * Navigates to the Vite-served `examples/browser.html`, which spawns a Web
 * Worker that hosts the WASM engine and runs two queries through the
 * worker-backed Database. The page writes the result of the second query
 * into `#out`; this test asserts that the round-trip succeeded without
 * blocking the main thread.
 */

import { test, expect } from "@playwright/test";

test("worker-backed Database: CREATE + MATCH round-trip in the browser", async ({
  page,
}) => {
  const logs: string[] = [];
  const errors: string[] = [];
  page.on("pageerror", (err) => errors.push(`pageerror: ${err.message}`));
  page.on("console", (msg) => {
    logs.push(`[${msg.type()}] ${msg.text()}`);
    if (msg.type() === "error") errors.push(`console.error: ${msg.text()}`);
  });
  page.on("requestfailed", (req) => {
    errors.push(`requestfailed: ${req.url()} — ${req.failure()?.errorText}`);
  });

  await page.goto("/examples/browser.html");

  // The page writes JSON into #out when the query returns. If the worker
  // bootstrap or the WASM call fails, the text starts with "error:".
  const out = page.locator("#out");
  try {
    await expect(out).not.toHaveText("running…", { timeout: 15_000 });
  } catch (e) {
    console.log("=== browser console log ===\n" + logs.join("\n"));
    console.log("=== browser errors ===\n" + errors.join("\n"));
    throw e;
  }
  const text = (await out.textContent()) ?? "";

  expect(
    text.startsWith("error:"),
    `page reported error: ${text}\nLOGS:\n${logs.join("\n")}\nERRORS:\n${errors.join("\n")}`,
  ).toBe(false);
  expect(errors, errors.join("\n")).toEqual([]);

  const parsed = JSON.parse(text) as { columns: string[]; rows: { name: string }[] };
  expect(parsed.columns).toEqual(["name"]);
  expect(parsed.rows).toEqual([{ name: "Alice" }]);
});
