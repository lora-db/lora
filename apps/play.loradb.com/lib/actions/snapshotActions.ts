"use client";

/**
 * Imperative actions that bridge the playground UI with the
 * `snapshots` IDB store and the WASM database. The Sidebar panel,
 * the New Snapshot dialog, and the Import button call these instead
 * of touching persistence or the database directly.
 *
 * Every IDB-mutating action ends with a `loradb:snapshots` window
 * event so the panel can refresh its in-memory list without polling.
 * `loadSnapshotById` additionally dispatches `loradb:mutation` so the
 * schema cache and DB-count chips refresh after a restore.
 */

import * as snapshots from "@/lib/persistence/snapshots";
import { loadSnapshot, saveSnapshot } from "@/lib/db/client";
import { LORADB_MUTATION_EVENT } from "@/lib/actions/runActiveTab";

export const SNAPSHOTS_EVENT = "loradb:snapshots";

function emitChange(): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new CustomEvent(SNAPSHOTS_EVENT));
}

function emitMutation(): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new CustomEvent(LORADB_MUTATION_EVENT));
}

/**
 * Serialise the current database state and persist it as a new
 * snapshot under `name`. Returns the freshly-created record.
 */
export async function createSnapshotFromDb(
  name: string,
): Promise<snapshots.Snapshot> {
  const blob = await saveSnapshot();
  const record = await snapshots.create({ name, blob });
  emitChange();
  return record;
}

/**
 * Restore a snapshot by id, replacing the live database contents.
 * Dispatches `loradb:mutation` so schema + counts refresh downstream.
 */
export async function loadSnapshotById(id: string): Promise<void> {
  const record = await snapshots.get(id);
  if (!record) {
    throw new Error(`Snapshot ${id} not found`);
  }
  await loadSnapshot(record.blob);
  emitMutation();
}

function sanitiseForFilename(name: string): string {
  // Replace anything non-alphanumeric/hyphen/underscore with `_` so the
  // browser download doesn't trip over user-supplied punctuation.
  return name.replace(/[^A-Za-z0-9._-]+/g, "_").slice(0, 80) || "snapshot";
}

function isoDateStamp(timestamp: number): string {
  // `YYYY-MM-DD` – sortable, locale-independent.
  return new Date(timestamp).toISOString().slice(0, 10);
}

/**
 * Trigger a browser download of the snapshot bytes as
 * `${name}-${date}.lorasnap`.
 */
export async function exportSnapshotToFile(id: string): Promise<void> {
  if (typeof window === "undefined") return;
  const record = await snapshots.get(id);
  if (!record) {
    throw new Error(`Snapshot ${id} not found`);
  }
  const filename = `${sanitiseForFilename(record.name)}-${isoDateStamp(
    record.createdAt,
  )}.lorasnap`;
  const blob = new Blob([new Uint8Array(record.blob)], {
    type: "application/octet-stream",
  });
  const url = URL.createObjectURL(blob);
  try {
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = filename;
    anchor.style.display = "none";
    document.body.appendChild(anchor);
    anchor.click();
    anchor.remove();
  } finally {
    // Give the browser a tick to start the download before revoking.
    setTimeout(() => {
      URL.revokeObjectURL(url);
    }, 1000);
  }
}

// Hard cap on imported `.lorasnap` payloads. The WASM engine can technically
// load larger blobs, but reading multi-hundred-MB ArrayBuffers into memory
// from a misclicked file is a much more common failure mode than a real
// snapshot of that size. Adjust upward only with a paired UX check.
const MAX_IMPORT_BYTES = 256 * 1024 * 1024;

/**
 * Read a user-picked `.lorasnap` file and persist its contents as a
 * new snapshot. Does NOT load the snapshot into the database — that
 * is the caller's call.
 */
export async function importSnapshotFromFile(
  file: File,
  name: string,
): Promise<snapshots.Snapshot> {
  if (file.size > MAX_IMPORT_BYTES) {
    const mb = (file.size / (1024 * 1024)).toFixed(1);
    throw new Error(
      `Snapshot file is ${mb} MB — exceeds the 256 MB import limit.`,
    );
  }
  const buffer = await file.arrayBuffer();
  const bytes = new Uint8Array(buffer);
  const record = await snapshots.create({ name, blob: bytes });
  emitChange();
  return record;
}

/** Delete a snapshot by id. */
export async function deleteSnapshotById(id: string): Promise<void> {
  await snapshots.remove(id);
  emitChange();
}
