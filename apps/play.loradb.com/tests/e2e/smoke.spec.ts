/**
 * Baseline smoke coverage: app boots, queries run, results render, and the
 * post-mutation count refresh kicks in.
 *
 * Anything failing here means the playground is fundamentally broken — the
 * other specs assume these basics work.
 */

import { expect, test } from "@playwright/test";

import {
  readStats,
  resetPlaygroundStorage,
  typeAndRun,
  waitForDbReady,
} from "./helpers";

test.describe("smoke", () => {
  test("app boots, DB reaches ready, default tab seeded", async ({ page }) => {
    await resetPlaygroundStorage(page);
    await expect(page).toHaveTitle(/LoraDB Playground/i);

    await waitForDbReady(page);

    // Default seed body lives in `Workbench`. The editor renders into a
    // contenteditable `.cm-content`; we just check the text is present.
    const editor = page.locator(".cm-content").first();
    await expect(editor).toBeVisible();
    await expect(editor).toContainText("MATCH");
  });

  test("simple RETURN renders in the Table tab", async ({ page }) => {
    await resetPlaygroundStorage(page);
    await waitForDbReady(page);

    await typeAndRun(page, "RETURN 1 AS n");

    // Result-pane stats line. We don't care exactly how many ms it took,
    // only that the success summary rendered.
    await expect
      .poll(async () => readStats(page), { timeout: 15_000 })
      .toMatch(/0 nodes · 0 rels · 1 rows · \d+ms/);

    // Status bar acknowledges one row.
    await expect(page.getByText(/· 1 row\b/)).toBeVisible();
  });

  test("CREATE mutates the DB and bumps the node count in the status bar", async ({
    page,
  }) => {
    await resetPlaygroundStorage(page);
    await waitForDbReady(page);

    await typeAndRun(
      page,
      "CREATE (a:Person {name: 'TestAlice'}) RETURN a",
    );

    // The post-mutation hook fires a DOM event that the status hook
    // listens to and refreshes counts asynchronously. Use `poll` to ride
    // out the refresh latency.
    await expect
      .poll(
        async () => {
          const text = await page
            .getByText(/· nodes \d+ · rels \d+/)
            .first()
            .textContent();
          return text ?? "";
        },
        { timeout: 15_000 },
      )
      .toMatch(/· nodes 1 · rels 0/);
  });
});
