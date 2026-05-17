/**
 * Share-URL round-trip: encode a query body into the `#q=` hash via the
 * playground's own lz-string module, navigate to that URL in a fresh
 * context, and assert the editor seeds with the decoded body.
 *
 * Clipboard reading inside a headless browser is brittle (browser context
 * permissions vary by Playwright version), so we sidestep it entirely:
 * `lz-string` is bundled into the page anyway, so we compute the hash via
 * `page.evaluate` and a dynamic import. That keeps the encoding scheme in
 * one place (the app's own dep tree) without requiring a Playwright-side
 * dep on lz-string.
 */

import { expect, test } from "@playwright/test";

import {
  resetPlaygroundStorage,
  waitForDbReady,
} from "./helpers";

const SHARED_BODY = "MATCH (n) RETURN count(n) AS total";

test.describe("share URL", () => {
  test("opening with #q=<encoded> seeds the editor with the decoded body", async ({
    page,
    baseURL,
  }) => {
    // Reset storage so we don't get a tab restored from auto-save.
    await resetPlaygroundStorage(page);
    await waitForDbReady(page);

    // Compute the share hash inside the page so we reuse the exact same
    // lz-string version the app bundles. We rely on Next.js' module graph
    // making `lz-string` resolvable at runtime — it's a direct dep of the
    // playground (see `apps/play.loradb.com/package.json`).
    const hash = await page.evaluate(async (body) => {
      const mod = (await import("lz-string")) as {
        default?: { compressToEncodedURIComponent(input: string): string };
        compressToEncodedURIComponent?: (input: string) => string;
      };
      const compress =
        mod.compressToEncodedURIComponent ??
        mod.default?.compressToEncodedURIComponent;
      if (!compress) {
        throw new Error("lz-string compressToEncodedURIComponent not found");
      }
      return compress(body);
    }, SHARED_BODY);

    expect(hash.length).toBeGreaterThan(0);

    // Open the share URL in a fresh page so we hit the cold-boot path
    // (mirrors a real recipient clicking the link).
    const url = `${baseURL ?? "http://localhost:4321"}/#q=${hash}`;
    const fresh = await page.context().newPage();
    await fresh.goto(url);

    // Wait for the DB & editor to bootstrap.
    const editor = fresh.locator(".cm-content").first();
    await expect(editor).toBeVisible({ timeout: 30_000 });

    // The editor body should match what we encoded.
    await expect(editor).toContainText(SHARED_BODY, { timeout: 15_000 });

    await fresh.close();
  });
});
