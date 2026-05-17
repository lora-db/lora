/**
 * Round-trip for the saved-queries panel: save the current tab, open a new
 * blank one, click the saved entry, expect the editor body to restore.
 */

import { expect, test } from "@playwright/test";

import {
  resetPlaygroundStorage,
  typeAndRun,
  waitForDbReady,
} from "./helpers";

const SAVED_BODY = "MATCH (n) RETURN n LIMIT 7";
const SAVED_NAME = "Test Saved Query";

test.describe("saved queries", () => {
  test("save current tab then re-open from the sidebar", async ({ page }) => {
    await resetPlaygroundStorage(page);
    await waitForDbReady(page);

    // Type the distinctive body. We intentionally do not run it — saving
    // doesn't depend on having a result.
    const editor = page.locator(".cm-content").first();
    await editor.click();
    await page.keyboard.press("ControlOrMeta+a");
    await page.keyboard.press("Delete");
    await page.keyboard.type(SAVED_BODY);

    // Open the Saved Queries panel from the Activity Bar.
    await page.getByRole("tab", { name: "Saved queries" }).click();

    // Save via the "Save current query" icon button in the panel header.
    await page.getByRole("button", { name: "Save current query" }).click();

    // SaveQueryDialog renders a TextInput labeled "Name".
    const nameInput = page.getByLabel("Name");
    await expect(nameInput).toBeVisible();
    await nameInput.fill(SAVED_NAME);
    await page.getByRole("button", { name: "Save" }).click();

    // Saved row should now appear in the panel.
    const savedRow = page.getByRole("button", {
      name: new RegExp(SAVED_NAME, "i"),
    });
    await expect(savedRow.first()).toBeVisible();

    // Replace the editor body with something else so the round-trip is
    // observable when we click the saved row.
    await editor.click();
    await page.keyboard.press("ControlOrMeta+a");
    await page.keyboard.press("Delete");
    await page.keyboard.type("RETURN 999 AS scratch");
    await expect(editor).toContainText("scratch");

    // Run that throwaway so the active tab definitely diverges.
    await typeAndRun(page, "RETURN 999 AS scratch");

    // Now click the saved row — `openSavedQuery` opens or focuses a tab
    // whose body is the saved one.
    await savedRow.first().click();

    // The active editor should now show the saved body.
    await expect(editor).toContainText("MATCH (n) RETURN n LIMIT 7");
  });
});
