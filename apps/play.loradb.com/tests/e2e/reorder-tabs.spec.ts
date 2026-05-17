/**
 * Tabs in the editor strip are draggable: dragging a tab onto another
 * slot reorders the list, and the new order survives a reload. The
 * persistence half is the load-bearing assertion — the store wires
 * `tabs[]` into the IDB session subscription, so as long as the order
 * mutates in place the reload should pick it up.
 */

import { expect, test } from "@playwright/test";

import { resetPlaygroundStorage, waitForDbReady } from "./helpers";

async function readTabNames(
  page: import("@playwright/test").Page,
): Promise<string[]> {
  const labels = await page
    .locator('[data-testid^="editor-tab-"]')
    .evaluateAll((els) =>
      els.map((el) => el.getAttribute("aria-label") ?? ""),
    );
  return labels.map((s) => s.replace(/^Activate tab /, ""));
}

test.describe("editor tab reordering", () => {
  test("drag reorders tabs and the order persists across reloads", async ({
    page,
  }) => {
    await resetPlaygroundStorage(page);
    await waitForDbReady(page);

    // Hydration seeds "Query 1"; press the "+" button twice to get
    // [Query 1, Query 2, Query 3].
    const newBtn = page.getByRole("button", { name: "New query tab" });
    for (let i = (await readTabNames(page)).length; i < 3; i++) {
      await newBtn.click();
    }
    const initial = await readTabNames(page);
    expect(initial).toEqual(["Query 1", "Query 2", "Query 3"]);

    const tabs = page.locator('[data-testid^="editor-tab-"]');
    const source = tabs.nth(0);
    const target = tabs.nth(2);
    const targetBox = await target.boundingBox();
    expect(targetBox).not.toBeNull();
    if (!targetBox) return;

    // Drop on the right half of the third tab so our handler treats this
    // as "insert after the third tab" — the dragged tab lands at the end.
    await source.dragTo(target, {
      targetPosition: { x: targetBox.width - 4, y: targetBox.height / 2 },
    });

    await expect
      .poll(() => readTabNames(page), { timeout: 5_000 })
      .toEqual(["Query 2", "Query 3", "Query 1"]);

    const reordered = await readTabNames(page);

    // The session.write subscription is debounced 500ms — give it room
    // before reloading.
    await page.waitForTimeout(700);
    await page.reload();
    await waitForDbReady(page);
    const afterReload = await readTabNames(page);
    expect(afterReload).toEqual(reordered);
  });

  test("keyboard chord moves the active tab left and right", async ({
    page,
  }) => {
    await resetPlaygroundStorage(page);
    await waitForDbReady(page);

    const newBtn = page.getByRole("button", { name: "New query tab" });
    for (let i = (await readTabNames(page)).length; i < 3; i++) {
      await newBtn.click();
    }
    expect(await readTabNames(page)).toEqual(["Query 1", "Query 2", "Query 3"]);

    // Active tab after seeding is the last one we opened ("Query 3"). Move
    // it left twice — should land at index 0.
    await page.keyboard.press("ControlOrMeta+Shift+Alt+ArrowLeft");
    await page.keyboard.press("ControlOrMeta+Shift+Alt+ArrowLeft");
    await expect
      .poll(() => readTabNames(page), { timeout: 2_000 })
      .toEqual(["Query 3", "Query 1", "Query 2"]);

    // Move it back right once.
    await page.keyboard.press("ControlOrMeta+Shift+Alt+ArrowRight");
    await expect
      .poll(() => readTabNames(page), { timeout: 2_000 })
      .toEqual(["Query 1", "Query 3", "Query 2"]);
  });
});
