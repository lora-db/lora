/**
 * CRUD wrapper around the `savedQueries` IDB store.
 *
 * Saved queries are the user's named query snippets — they survive page
 * reloads and snapshot rebuilds, and can be opened into a fresh editor tab.
 * Listing returns newest-modified first via the `byUpdatedAt` index.
 */

import { ulid } from "@/lib/util/id";
import { getDB } from "./idb";

export interface SavedQuery {
  id: string;
  name: string;
  body: string;
  /** Raw JSON source for the `$param` payload. Default `"{}"`. */
  params: string;
  tags: string[];
  createdAt: number;
  updatedAt: number;
}

/** Default empty payload — kept as a constant so call sites are explicit. */
export const DEFAULT_PARAMS = "{}";

function normalize(raw: SavedQuery): SavedQuery {
  if (typeof raw.params === "string") return raw;
  return { ...raw, params: DEFAULT_PARAMS };
}

/** Returns all saved queries, most recently updated first. */
export async function list(): Promise<SavedQuery[]> {
  const db = await getDB();
  // `getAllFromIndex` returns ascending by index value; reverse for newest-first.
  const all = await db.getAllFromIndex("savedQueries", "byUpdatedAt");
  return all.reverse().map(normalize);
}

export async function get(id: string): Promise<SavedQuery | undefined> {
  const db = await getDB();
  const raw = await db.get("savedQueries", id);
  return raw ? normalize(raw) : undefined;
}

export async function create(input: {
  name: string;
  body: string;
  params?: string;
  tags?: string[];
}): Promise<SavedQuery> {
  const now = Date.now();
  const record: SavedQuery = {
    id: ulid(),
    name: input.name,
    body: input.body,
    params: input.params ?? DEFAULT_PARAMS,
    tags: input.tags ?? [],
    createdAt: now,
    updatedAt: now,
  };
  const db = await getDB();
  await db.put("savedQueries", record);
  return record;
}

export async function update(
  id: string,
  patch: Partial<Pick<SavedQuery, "name" | "body" | "params" | "tags">>,
): Promise<SavedQuery> {
  const db = await getDB();
  const existing = await db.get("savedQueries", id);
  if (!existing) {
    throw new Error(`SavedQuery ${id} not found`);
  }
  const next: SavedQuery = normalize({
    ...existing,
    ...patch,
    tags: patch.tags ?? existing.tags,
    updatedAt: Date.now(),
  });
  await db.put("savedQueries", next);
  return next;
}

export async function remove(id: string): Promise<void> {
  const db = await getDB();
  await db.delete("savedQueries", id);
}
