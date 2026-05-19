/**
 * CRUD wrapper around the `snapshots` IDB store.
 *
 * Snapshots hold serialized LoraDB database blobs the user has chosen to
 * keep around. The list view only needs metadata, so `list()` strips the
 * (potentially large) `blob` field before returning.
 */

import { ulid } from "@/lib/util/id";
import { getDB } from "./idb";

/**
 * Header context decoded from the snapshot binary at create/import time.
 * Mirrors the WASM `SnapshotInfo` shape minus the redundant `walLsn` byte
 * count fields renamed for storage. `null` on legacy records persisted
 * before this field was introduced.
 */
export interface SnapshotHeader {
  /** On-disk format version. */
  formatVersion: number;
  /** Node count recorded in the manifest. */
  nodeCount: number;
  /** Relationship count recorded in the manifest. */
  relationshipCount: number;
  /** WAL fence LSN if the snapshot was a checkpoint (always `null` from the
   * playground today — we save without a WAL). */
  walLsn: number | null;
  /** Body codec. `gzip` carries the encoder level. */
  compression: { format: "none" } | { format: "gzip"; level: number };
  /** Whether the body is encrypted. The blob is unreadable without
   * credentials when `true`. */
  encrypted: boolean;
  /** Key identifier the snapshot was sealed with, if any — used as a hint
   * in the passphrase prompt on load. */
  keyId: string | null;
}

export interface Snapshot {
  id: string;
  name: string;
  blob: Uint8Array;
  sizeBytes: number;
  createdAt: number;
  /** Header parsed from the snapshot bytes. Missing on records persisted
   * before this field was introduced — callers should treat as optional. */
  header?: SnapshotHeader;
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
  header?: SnapshotHeader;
}): Promise<Snapshot> {
  const record: Snapshot = {
    id: ulid(),
    name: input.name,
    blob: input.blob,
    sizeBytes: input.blob.byteLength,
    createdAt: Date.now(),
    ...(input.header ? { header: input.header } : {}),
  };
  const db = await getDB();
  await db.put("snapshots", record);
  return record;
}

export async function remove(id: string): Promise<void> {
  const db = await getDB();
  await db.delete("snapshots", id);
}
