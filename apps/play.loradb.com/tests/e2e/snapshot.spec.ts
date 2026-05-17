/**
 * Snapshot create / reset / restore round-trip. Verifies the WASM DB state
 * really is mutated, persisted into IDB, and restored byte-for-byte (well,
 * node-for-node — we just check the count and a property lookup).
 */

import { expect, test, type Page } from "@playwright/test";

import {
  readStats,
  resetPlaygroundStorage,
  typeAndRun,
  waitForDbReady,
} from "./helpers";

const SNAP_NAME = "first-snap";

async function expectNodeCountInStatusBar(
  page: Page,
  count: number,
): Promise<void> {
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
    .toMatch(new RegExp(`· nodes ${count} · rels 0`));
}

test.describe("snapshots", () => {
  test("create, wipe, reload — node restored", async ({ page }) => {
    await resetPlaygroundStorage(page);
    await waitForDbReady(page);

    // Seed one node.
    await typeAndRun(page, "CREATE (a:Snap {n: 'X'})");
    await expectNodeCountInStatusBar(page, 1);

    // Open the snapshots panel and create a snapshot.
    await page.getByRole("tab", { name: "Snapshots" }).click();
    await page.getByRole("button", { name: "New snapshot" }).click();

    const nameInput = page.getByLabel("Name");
    await expect(nameInput).toBeVisible();
    await nameInput.fill(SNAP_NAME);
    await page.getByRole("button", { name: "Create" }).click();

    // Snapshot row should appear.
    const snapRow = page.getByRole("button", {
      name: new RegExp(SNAP_NAME, "i"),
    });
    await expect(snapRow.first()).toBeVisible();

    // Wipe the DB.
    await typeAndRun(page, "MATCH (n) DETACH DELETE n");
    await expectNodeCountInStatusBar(page, 0);

    // Click the snapshot to load it. `SnapshotsPanel` always asks via
    // `openConfirmModal`, so we accept-or-skip both branches.
    await snapRow.first().click();
    const confirmBtn = page.getByRole("button", { name: "Load" }).last();
    if (await confirmBtn.isVisible().catch(() => false)) {
      await confirmBtn.click();
    }

    // Node count should bounce back to 1.
    await expectNodeCountInStatusBar(page, 1);

    // And the property is intact.
    await typeAndRun(page, "MATCH (n) WHERE n.n = 'X' RETURN n");
    await expect
      .poll(async () => readStats(page), { timeout: 15_000 })
      .toMatch(/1 nodes · 0 rels · 1 rows · \d+ms/);
  });
});
