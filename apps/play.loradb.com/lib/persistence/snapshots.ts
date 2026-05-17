/**
 * CRUD wrapper around the `snapshots` IDB store.
 *
 * Snapshots hold serialized LoraDB database blobs the user has chosen to
 * keep around. The list view only needs metadata, so `list()` strips the
 * (potentially large) `blob` field before returning.
 */

import { ulid } from "@/lib/util/id";
import { getDB } from "./idb";

export interface Snapshot {
  id: string;
  name: string;
  blob: Uint8Array;
  sizeBytes: number;
  createdAt: number;
}

export type SnapshotMeta = Omit<Snapshot, "blob">;

/** Returns metadata for all snapshots, newest first. Excludes the blob bytes. */
export async function list(): Promise<SnapshotMeta[]> {
  const db = await getDB();
  const all = await db.getAll("snapshots");
  return all
    .map((row) => {
      const { blob, ...meta } = row;
      void blob;
      return meta;
    })
    .sort((a, b) => b.createdAt - a.createdAt);
}

export async function get(id: string): Promise<Snapshot | undefined> {
  const db = await getDB();
  return db.get("snapshots", id);
}

export async function create(input: {
  name: string;
  blob: Uint8Array;
}): Promise<Snapshot> {
  const record: Snapshot = {
    id: ulid(),
    name: input.name,
    blob: input.blob,
    sizeBytes: input.blob.byteLength,
    createdAt: Date.now(),
  };
  const db = await getDB();
  await db.put("snapshots", record);
  return record;
}

export async function remove(id: string): Promise<void> {
  const db = await getDB();
  await db.delete("snapshots", id);
}
