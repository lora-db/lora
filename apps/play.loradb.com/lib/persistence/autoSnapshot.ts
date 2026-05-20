"use client";

/**
 * Auto-restore snapshot storage.
 *
 * Stashes a single serialized DB blob in IndexedDB so the workbench can
 * rehydrate after a page reload. The blob lives in its own `autoSnapshot`
 * object store (singleton row) and is deliberately kept out of the
 * `snapshots` store so it stays invisible to the user-facing Snapshots
 * panel. IDB stores the `Uint8Array` natively — no base64 round-trip — and
 * gives us orders-of-magnitude more headroom than the ~5 MB localStorage
 * cap we used previously.
 */

import { getDB } from "./idb";

const STORE = "autoSnapshot";
const KEY = "singleton";

// Soft cap to avoid pathological cases (e.g. a multi-GB graph that would
// make every flush take ages and starve the UI). IDB itself can hold much
// more, but at this size the user is better served by an explicit
// user-named snapshot they can manage.
const MAX_BYTES = 256 * 1024 * 1024;

export interface AutoSnapshotRecord {
  id: string;
  blob: Uint8Array;
  savedAt: number;
}

/**
 * Per-write outcome. `quota-exceeded` is distinguished from `too-large`
 * so the UI can give the user a more actionable hint (free up space vs.
 * use a manual snapshot).
 */
export type WriteResult =
  | { ok: true }
  | { ok: false; reason: "too-large"; size: number; cap: number }
  | { ok: false; reason: "quota-exceeded" }
  | { ok: false; reason: "unavailable" };

/** Read the auto-snapshot from IDB. Returns `null` when no entry exists. */
export async function readAuto(): Promise<Uint8Array | null> {
  if (typeof window === "undefined") return null;
  try {
    const db = await getDB();
    const row = await db.get(STORE, KEY);
    return row ? row.blob : null;
  } catch (err) {
    console.warn("readAuto: IDB access failed", err);
    return null;
  }
}

export async function writeAuto(blob: Uint8Array): Promise<WriteResult> {
  if (typeof window === "undefined") {
    return { ok: false, reason: "unavailable" };
  }
  if (blob.byteLength > MAX_BYTES) {
    console.warn(
      `autoSnapshot: blob is ${blob.byteLength}B, exceeds the ${MAX_BYTES}B cap — skipping`,
    );
    return {
      ok: false,
      reason: "too-large",
      size: blob.byteLength,
      cap: MAX_BYTES,
    };
  }
  try {
    const db = await getDB();
    const record: AutoSnapshotRecord = {
      id: KEY,
      blob,
      savedAt: Date.now(),
    };
    await db.put(STORE, record);
    return { ok: true };
  } catch (err) {
    if (err instanceof DOMException && err.name === "QuotaExceededError") {
      console.warn("writeAuto: browser storage quota exceeded", err);
      return { ok: false, reason: "quota-exceeded" };
    }
    console.warn("writeAuto failed", err);
    return { ok: false, reason: "unavailable" };
  }
}

export async function clearAuto(): Promise<void> {
  if (typeof window === "undefined") return;
  try {
    const db = await getDB();
    await db.delete(STORE, KEY);
  } catch (err) {
    console.warn("clearAuto: IDB delete failed", err);
  }
}

/** Exposed for the UI so it can phrase the cap in toasts/help text. */
export const AUTO_SNAPSHOT_CAP_BYTES = MAX_BYTES;
