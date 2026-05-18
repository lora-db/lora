/**
 * Window-management e2e coverage.
 *
 * Exercises the recursive workspace tree end-to-end: orientation
 * toggle, split right, split down, close pane, persistence across
 * reloads. These tests target visible UI affordances (toolbar + buttons
 * inside the leaf header) so they fail in a way that matches what a
 * user would experience.
 */

import { expect, test } from "@playwright/test";

import {
  resetPlaygroundStorage,
  waitForDbReady,
} from "./helpers";

async function paneCount(page: import("@playwright/test").Page): Promise<number> {
  return await page.locator("[data-pane-id]").count();
}

test.describe("window management", () => {
  test("default workspace renders one editor leaf and one result leaf", async ({
    page,
  }) => {
    await resetPlaygroundStorage(page);
    await waitForDbReady(page);
    await expect.poll(() => paneCount(page)).toBeGreaterThanOrEqual(2);
    // Editor view chip is present.
    await expect(
      page.locator('[data-view-kind="editor"]').first(),
    ).toBeVisible();
    await expect(
      page.locator('[data-view-kind="result"]').first(),
    ).toBeVisible();
  });

  test("toggling orientation flips the root group direction", async ({
    page,
  }) => {
    await resetPlaygroundStorage(page);
    await waitForDbReady(page);

    // Capture the root group's data-orientation attribute (set by
    // react-resizable-panels). The Group component renders a div with
    // a known `data-group` data attribute carrying the orientation.
    const groupOrientation = async () => {
      return await page.evaluate(() => {
        const el = document.querySelector("[data-orientation]");
        return el ? el.getAttribute("data-orientation") : null;
      });
    };

    const before = await groupOrientation();
    await page.getByTestId("toggle-orientation").click();
    await expect.poll(groupOrientation).not.toBe(before);
  });

  test("split right adds a second pane that can be closed", async ({
    page,
  }) => {
    await resetPlaygroundStorage(page);
    await waitForDbReady(page);

    const initial = await paneCount(page);
    // Click the split-right button on the editor leaf (the first one).
    await page
      .locator('[data-pane-id]')
      .first()
      .getByRole("button", { name: "Split right" })
      .click();

    await expect.poll(() => paneCount(page)).toBeGreaterThan(initial);

    // Closing the new pane brings us back.
    await page
      .locator('[data-pane-id]')
      .last()
      .getByRole("button", { name: "Close pane" })
      .click();

    await expect.poll(() => paneCount(page)).toBe(initial);
  });

  test("layout persists across reload", async ({ page }) => {
    await resetPlaygroundStorage(page);
    await waitForDbReady(page);

    // Split twice to get three panes.
    await page
      .locator('[data-pane-id]')
      .first()
      .getByRole("button", { name: "Split right" })
      .click();
    await page
      .locator('[data-pane-id]')
      .first()
      .getByRole("button", { name: "Split down" })
      .click();

    await expect.poll(() => paneCount(page)).toBe(4);

    // Reload — the layout slice persists `workspace` so the tree should
    // return with four leaves intact.
    await page.reload();
    await waitForDbReady(page);
    await expect.poll(() => paneCount(page)).toBe(4);
  });
});
