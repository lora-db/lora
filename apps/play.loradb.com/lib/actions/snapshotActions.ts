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

import type {
  SnapshotInfo,
  WasmSnapshotEncryption,
  WasmSnapshotLoadOptions,
} from "@loradb/lora-wasm";

import * as snapshots from "@/lib/persistence/snapshots";
import { loadSnapshot, readSnapshotInfo, saveSnapshot } from "@/lib/db/client";
import { LORADB_MUTATION_EVENT } from "@/lib/actions/runActiveTab";

/**
 * Optional protection applied to a new snapshot. Currently a passphrase —
 * the same passphrase must be supplied at load time. `keyId` is opaque
 * metadata shown in the load prompt as a hint to the user.
 */
export interface SnapshotProtection {
  password: string;
  keyId?: string;
}

function infoToHeader(info: SnapshotInfo): snapshots.SnapshotHeader {
  return {
    formatVersion: info.formatVersion,
    nodeCount: info.nodeCount,
    relationshipCount: info.relationshipCount,
    walLsn: info.walLsn,
    compression: info.compression,
    encrypted: info.encrypted,
    keyId: info.keyId,
  };
}

function encryptionFromProtection(
  protection: SnapshotProtection | undefined,
): WasmSnapshotEncryption | undefined {
  if (!protection) return undefined;
  return {
    type: "password",
    keyId: protection.keyId ?? "playground",
    password: protection.password,
  };
}

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
 * snapshot under `name`. Optionally seals the body with a passphrase
 * (ChaCha20-Poly1305 + Argon2 KDF) — loading later requires the same
 * passphrase. Returns the freshly-created record with the parsed
 * header attached.
 */
export async function createSnapshotFromDb(
  name: string,
  protection?: SnapshotProtection,
): Promise<snapshots.Snapshot> {
  const encryption = encryptionFromProtection(protection);
  const blob = await saveSnapshot(encryption ? { encryption } : undefined);
  const header = infoToHeader(await readSnapshotInfo(blob));
  const record = await snapshots.create({ name, blob, header });
  emitChange();
  return record;
}

/** Error thrown by `loadSnapshotById` when the stored snapshot needs a
 * passphrase but the caller did not supply one. The Sidebar uses this to
 * route the user into the passphrase prompt. */
export class SnapshotPasswordRequiredError extends Error {
  readonly keyId: string | null;
  constructor(keyId: string | null) {
    super("Snapshot is encrypted — passphrase required");
    this.name = "SnapshotPasswordRequiredError";
    this.keyId = keyId;
  }
}

/**
 * Restore a snapshot by id, replacing the live database contents.
 * Dispatches `loradb:mutation` so schema + counts refresh downstream.
 *
 * For encrypted snapshots, supply `protection` with the passphrase the
 * snapshot was sealed with. Omitting it raises
 * {@link SnapshotPasswordRequiredError} so the caller can prompt the user.
 */
export async function loadSnapshotById(
  id: string,
  protection?: SnapshotProtection,
): Promise<void> {
  const record = await snapshots.get(id);
  if (!record) {
    throw new Error(`Snapshot ${id} not found`);
  }
  // Prefer the persisted header but fall back to re-parsing the blob if
  // the record predates header storage.
  const encrypted = record.header
    ? record.header.encrypted
    : (await readSnapshotInfo(record.blob)).encrypted;
  if (encrypted && !protection) {
    throw new SnapshotPasswordRequiredError(record.header?.keyId ?? null);
  }
  const opts: WasmSnapshotLoadOptions | undefined = protection
    ? {
        credentials: {
          type: "password",
          keyId: protection.keyId ?? record.header?.keyId ?? "playground",
          password: protection.password,
        },
      }
    : undefined;
  await loadSnapshot(record.blob, opts);
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
  // Parse the envelope eagerly so encryption/compression are visible in
  // the panel without re-decoding the bytes on every render. A malformed
  // file surfaces a typed error here rather than at load time.
  const header = infoToHeader(await readSnapshotInfo(bytes));
  const record = await snapshots.create({ name, blob: bytes, header });
  emitChange();
  return record;
}

/** Delete a snapshot by id. */
export async function deleteSnapshotById(id: string): Promise<void> {
  await snapshots.remove(id);
  emitChange();
}
