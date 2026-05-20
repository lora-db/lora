/**
 * CRUD wrapper around the `history` IDB store.
 *
 * Every query run (success or failure) gets appended here so the user can
 * scrub through their session. The store is bounded at `MAX_ENTRIES` —
 * once exceeded, the oldest entries are evicted in the same transaction
 * that wrote the new one.
 */

import { ulid } from "@/lib/util/id";
import { getDB } from "./idb";

export interface HistoryEntry {
  id: string;
  tabId?: string;
  body: string;
  /** Raw JSON source of the `$param` payload used for this run. Default `"{}"`. */
  params: string;
  startedAt: number;
  ms: number;
  rowCount: number;
  ok: boolean;
  errorMessage?: string;
}

/** Default empty payload — kept as a constant so call sites are explicit. */
export const DEFAULT_PARAMS = "{}";

function normalize(raw: HistoryEntry): HistoryEntry {
  if (typeof raw.params === "string") return raw;
  return { ...raw, params: DEFAULT_PARAMS };
}

const MAX_ENTRIES = 1000;
const DEFAULT_LIMIT = 200;

/** Returns history entries newest-first, capped at `limit` (default 200). */
export async function list(
  limit: number = DEFAULT_LIMIT,
): Promise<HistoryEntry[]> {
  const db = await getDB();
  const all = await db.getAllFromIndex("history", "byStartedAt");
  all.reverse();
  return all.slice(0, Math.max(0, limit)).map(normalize);
}

/**
 * Appends one entry and trims the store back down to `MAX_ENTRIES` by
 * deleting the oldest entries (by `startedAt`) inside the same transaction.
 *
 * Dedup: if a prior entry has the same `body` + `params`, its id is
 * reused and the new fields (startedAt, ms, rowCount, ok, errorMessage)
 * overwrite the old record. Because the panel sorts by `startedAt`
 * newest-first, the re-run naturally bubbles to the top — the history
 * behaves like a shell with `HIST_IGNORE_ALL_DUPS`.
 */
export async function append(
  entry: Omit<HistoryEntry, "id" | "params"> & { params?: string },
): Promise<HistoryEntry> {
  const params = entry.params ?? DEFAULT_PARAMS;
  const db = await getDB();
  const tx = db.transaction("history", "readwrite");
  const store = tx.store;

  // Walk newest-first so the common case (re-running the most recent
  // query) hits on iteration one.
  let existingId: string | undefined;
  let dupCursor = await store.index("byStartedAt").openCursor(null, "prev");
  while (dupCursor) {
    const value = dupCursor.value;
    if (value.body === entry.body && value.params === params) {
      existingId = value.id;
      break;
    }
    dupCursor = await dupCursor.continue();
  }

  const record: HistoryEntry = {
    id: existingId ?? ulid(),
    ...entry,
    params,
  };
  await store.put(record);

  // Trim oldest if we exceeded the cap. Use the `byStartedAt` index cursor
  // ascending so we delete in oldest-first order.
  const count = await store.count();
  if (count > MAX_ENTRIES) {
    const toDelete = count - MAX_ENTRIES;
    const index = store.index("byStartedAt");
    let cursor = await index.openCursor();
    let deleted = 0;
    while (cursor && deleted < toDelete) {
      await cursor.delete();
      deleted += 1;
      cursor = await cursor.continue();
    }
  }

  await tx.done;
  return record;
}

/** Empties the history store. */
export async function clear(): Promise<void> {
  const db = await getDB();
  await db.clear("history");
}
