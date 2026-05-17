/**
 * Spotlight palette: opens on Cmd/Ctrl-K, lists actions, and "Run query"
 * actually runs the active editor body.
 */

import { expect, test } from "@playwright/test";

import {
  openSpotlight,
  readStats,
  resetPlaygroundStorage,
  waitForDbReady,
} from "./helpers";

test.describe("spotlight", () => {
  test("opens on Cmd/Ctrl-K and lists actions", async ({ page }) => {
    await resetPlaygroundStorage(page);
    await waitForDbReady(page);

    await openSpotlight(page);

    // Mantine Spotlight's search field uses our custom placeholder.
    const search = page.getByPlaceholder("Search commands...");
    await expect(search).toBeVisible();

    // `SpotlightHost` registers > 10 actions. Mantine renders each as a
    // button inside the action list — counting `button` roles inside the
    // dialog gives us a stable lower bound.
    const dialog = page.getByRole("dialog");
    const actionCount = await dialog.getByRole("button").count();
    expect(actionCount).toBeGreaterThanOrEqual(5);
  });

  test("Run query action executes the active tab", async ({ page }) => {
    await resetPlaygroundStorage(page);
    await waitForDbReady(page);

    // Replace the active body with a deterministic, non-empty RETURN.
    const editor = page.locator(".cm-content").first();
    await editor.click();
    await page.keyboard.press("ControlOrMeta+a");
    await page.keyboard.press("Delete");
    await page.keyboard.type("RETURN 42 AS answer");

    await openSpotlight(page);
    const search = page.getByPlaceholder("Search commands...");
    await expect(search).toBeVisible();
    await search.fill("Run query");
    // Enter selects the highlighted match.
    await page.keyboard.press("Enter");

    // The result pane should now render the stats summary.
    await expect
      .poll(async () => readStats(page), { timeout: 15_000 })
      .toMatch(/0 nodes · 0 rels · 1 rows · \d+ms/);
  });
});
