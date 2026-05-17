/**
 * Shared helpers for the LoraDB playground e2e suite.
 *
 * Each helper is deliberately small and idempotent so individual spec files
 * stay narrative — the test reads like a user story, not a Playwright
 * tutorial. Anything that touches state the playground persists across
 * reloads (IndexedDB, localStorage) lives behind `resetPlaygroundStorage`.
 */

import { expect, type Page } from "@playwright/test";

/** Polls the status bar until it reports `db: ready`. */
export async function waitForDbReady(page: Page): Promise<void> {
  // The bottom status bar always renders "db: <state>". When the WASM boot
  // completes the state flips to `ready`. We anchor on the "db:" prefix and
  // wait for the trailing word.
  const dbStatus = page.getByText(/^db:\s*ready$/i);
  await expect(dbStatus).toBeVisible({ timeout: 30_000 });
}

/**
 * Replace the active editor's body with `body` and run it.
 *
 * The editor is CodeMirror (contenteditable, not a textarea), so we set the
 * value through Playwright's `fill` on the `.cm-content` node — CM observes
 * input events and rebuilds its internal state from them. After typing we
 * dispatch the run via `mod+Enter` (Mantine's `useHotkeys` is what wires
 * that shortcut up in `EditorPane`).
 */
export async function typeAndRun(page: Page, body: string): Promise<void> {
  const editor = page.locator(".cm-content").first();
  await editor.waitFor({ state: "visible" });
  await editor.click();
  // Clear any existing content first. ControlOrMeta picks the correct
  // modifier for the host platform.
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.press("Delete");
  // `pressSequentially` actually emits per-character `input` events so
  // CodeMirror's transactions fire. `fill` short-circuits on
  // contenteditable in some Playwright versions, so we avoid it here.
  await page.keyboard.type(body);
  // Give CM one paint to flush the document → state.body sync before run.
  await page.waitForTimeout(50);
  await page.keyboard.press("ControlOrMeta+Enter");
}

/**
 * Reads the result-pane stats line ("N nodes · N rels · N rows · Nms") if
 * the active result is in the ok state. Returns null otherwise.
 */
export async function readStats(page: Page): Promise<string | null> {
  const stats = page.locator("text=/\\d+ nodes · \\d+ rels · \\d+ rows · \\d+ms/");
  if ((await stats.count()) === 0) return null;
  return (await stats.first().textContent())?.trim() ?? null;
}

/** Open the Mantine Spotlight palette. */
export async function openSpotlight(page: Page): Promise<void> {
  // Click the body first so the focus is outside CodeMirror; otherwise the
  // editor swallows the chord (Mantine's `useHotkeys` is global, but CM
  // pre-empts on some chords).
  await page.locator("body").click({ position: { x: 1, y: 1 } });
  await page.keyboard.press("ControlOrMeta+k");
}

/**
 * Nukes the playground's IndexedDB + localStorage and reloads. Tests call
 * this first thing so they start from a known-empty state.
 */
export async function resetPlaygroundStorage(page: Page): Promise<void> {
  await page.goto("/");
  await page.evaluate(async () => {
    try {
      localStorage.clear();
    } catch {
      /* ignore */
    }
    try {
      sessionStorage.clear();
    } catch {
      /* ignore */
    }
    // Wait for IDB deletion so the next reload sees a clean slate.
    await new Promise<void>((resolve) => {
      const req = indexedDB.deleteDatabase("loradb-play");
      req.onsuccess = () => {
        resolve();
      };
      req.onerror = () => {
        resolve();
      };
      req.onblocked = () => {
        resolve();
      };
    });
  });
  await page.reload();
}
