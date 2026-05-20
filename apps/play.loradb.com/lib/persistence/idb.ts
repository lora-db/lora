/**
 * Typed IndexedDB wrapper for the LoraDB playground.
 *
 * Single database (`loradb-play`) with five object stores: saved queries,
 * snapshots (binary blobs), run history, the workbench session record, and
 * a meta store reserved for future schema migrations. All stores share a
 * single integer `version` field on the IDB database itself.
 *
 * The database handle is created lazily on first access and cached as a
 * module-level singleton. Server-side calls throw immediately — IDB is a
 * browser API and the playground is a fully client-rendered surface.
 */

import { openDB, deleteDB, type DBSchema, type IDBPDatabase } from "idb";

import type { SavedQuery } from "./savedQueries";
import type { Snapshot } from "./snapshots";
import type { HistoryEntry } from "./history";
import type { SessionRecord } from "./session";
import type { AutoSnapshotRecord } from "./autoSnapshot";

const DB_NAME = "loradb-play";
// IMPORTANT: bump DB_VERSION (and add a versioned branch in `upgrade`)
// for EVERY schema change — new store, new index, renamed key path. The
// current `upgrade` is intentionally idempotent (`!contains` guards) so
// fresh installs Just Work, but that guard does NOT migrate existing
// users' data when the shape changes. Adding a store without bumping the
// version will leave returning users without that store and crash any
// read against it.
const DB_VERSION = 2;

export interface PlayDB extends DBSchema {
  savedQueries: {
    key: string;
    value: SavedQuery;
    indexes: {
      byName: string;
      byUpdatedAt: number;
    };
  };
  snapshots: {
    key: string;
    value: Snapshot;
    indexes: {
      byName: string;
    };
  };
  history: {
    key: string;
    value: HistoryEntry;
    indexes: {
      byStartedAt: number;
    };
  };
  session: {
    key: string;
    value: SessionRecord;
  };
  meta: {
    key: string;
    value: { id: "singleton"; [k: string]: unknown };
  };
  autoSnapshot: {
    key: string;
    value: AutoSnapshotRecord;
  };
}

let dbPromise: Promise<IDBPDatabase<PlayDB>> | null = null;

function assertClient(): void {
  if (typeof window === "undefined") {
    throw new Error("loradb-play IDB is only available in the browser");
  }
}

function init(): Promise<IDBPDatabase<PlayDB>> {
  return openDB<PlayDB>(DB_NAME, DB_VERSION, {
    upgrade(db) {
      if (!db.objectStoreNames.contains("savedQueries")) {
        const store = db.createObjectStore("savedQueries", { keyPath: "id" });
        store.createIndex("byName", "name", { unique: false });
        store.createIndex("byUpdatedAt", "updatedAt");
      }
      if (!db.objectStoreNames.contains("snapshots")) {
        const store = db.createObjectStore("snapshots", { keyPath: "id" });
        store.createIndex("byName", "name");
      }
      if (!db.objectStoreNames.contains("history")) {
        const store = db.createObjectStore("history", { keyPath: "id" });
        store.createIndex("byStartedAt", "startedAt");
      }
      if (!db.objectStoreNames.contains("session")) {
        db.createObjectStore("session", { keyPath: "id" });
      }
      if (!db.objectStoreNames.contains("meta")) {
        db.createObjectStore("meta", { keyPath: "id" });
      }
      if (!db.objectStoreNames.contains("autoSnapshot")) {
        db.createObjectStore("autoSnapshot", { keyPath: "id" });
      }
    },
  });
}

/** Returns the (lazy, cached) IDB handle. Throws if called server-side. */
export function getDB(): Promise<IDBPDatabase<PlayDB>> {
  assertClient();
  if (!dbPromise) {
    dbPromise = init();
  }
  return dbPromise;
}

/** Deletes the entire playground database and re-initialises a fresh one. */
export async function resetDB(): Promise<void> {
  assertClient();
  if (dbPromise) {
    const db = await dbPromise;
    db.close();
    dbPromise = null;
  }
  await deleteDB(DB_NAME);
  dbPromise = init();
  await dbPromise;
}
