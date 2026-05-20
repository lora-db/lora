/**
 * Single-record session persistence — the open tabs, the layout state and
 * the user preferences blob. Read once on mount, written (debounced) by the
 * store on every relevant state change.
 *
 * The `SerializedTab`/`SerializedLayout`/`SerializedPrefs` shapes mirror what
 * the corresponding slices export; we re-import them here so a single source
 * of truth (the slice) still owns the schema definitions.
 */

import { getDB } from "./idb";

import type { SerializedTab } from "@/lib/state/slices/tabs";
import type { SerializedLayout } from "@/lib/state/slices/layout";
import type { SerializedPrefs } from "@/lib/state/slices/prefs";

export type { SerializedTab, SerializedLayout, SerializedPrefs };

export interface SessionRecord {
  id: "singleton";
  tabs: SerializedTab[];
  /**
   * Legacy field — the active tab id used to live here. The current
   * model derives "active tab" from the workspace tree, so this field
   * is preserved as `null` on new writes for backward compatibility
   * with older readers, and consulted as a one-shot hint on hydrate.
   */
  activeTabId: string | null;
  layout: SerializedLayout;
  prefs: SerializedPrefs;
  updatedAt: number;
}

/**
 * Reads the singleton session record, or `undefined` on first run.
 * Returns `undefined` (not throws) on IDB failure — the workbench falls
 * back to a fresh session rather than crashing the UI.
 */
export async function read(): Promise<SessionRecord | undefined> {
  try {
    const db = await getDB();
    return await db.get("session", "singleton");
  } catch (err) {
    console.warn(
      "session.read failed; starting with no persisted session",
      err,
    );
    return undefined;
  }
}

/**
 * Writes the singleton session record, stamping `updatedAt` automatically.
 * Returns `true` on success and `false` on any IDB failure (quota exceeded,
 * storage disabled, version-mismatch, etc.) so callers can surface a
 * one-shot notification.
 */
export async function write(
  record: Omit<SessionRecord, "id" | "updatedAt">,
): Promise<boolean> {
  const full: SessionRecord = {
    id: "singleton",
    ...record,
    updatedAt: Date.now(),
  };
  try {
    const db = await getDB();
    await db.put("session", full);
    return true;
  } catch (err) {
    console.warn("session.write failed", err);
    return false;
  }
}
