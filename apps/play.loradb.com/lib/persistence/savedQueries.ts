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
  tags: string[];
  createdAt: number;
  updatedAt: number;
}

/** Returns all saved queries, most recently updated first. */
export async function list(): Promise<SavedQuery[]> {
  const db = await getDB();
  // `getAllFromIndex` returns ascending by index value; reverse for newest-first.
  const all = await db.getAllFromIndex("savedQueries", "byUpdatedAt");
  return all.reverse();
}

export async function get(id: string): Promise<SavedQuery | undefined> {
  const db = await getDB();
  return db.get("savedQueries", id);
}

export async function create(input: {
  name: string;
  body: string;
  tags?: string[];
}): Promise<SavedQuery> {
  const now = Date.now();
  const record: SavedQuery = {
    id: ulid(),
    name: input.name,
    body: input.body,
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
  patch: Partial<Pick<SavedQuery, "name" | "body" | "tags">>,
): Promise<SavedQuery> {
  const db = await getDB();
  const existing = await db.get("savedQueries", id);
  if (!existing) {
    throw new Error(`SavedQuery ${id} not found`);
  }
  const next: SavedQuery = {
    ...existing,
    ...patch,
    // Preserve the existing tags array if the patch didn't provide one.
    tags: patch.tags ?? existing.tags,
    updatedAt: Date.now(),
  };
  await db.put("savedQueries", next);
  return next;
}

export async function remove(id: string): Promise<void> {
  const db = await getDB();
  await db.delete("savedQueries", id);
}
