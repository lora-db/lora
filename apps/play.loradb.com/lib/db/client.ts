"use client";

/**
 * Client-only wrapper around the `@loradb/lora-wasm` Database singleton.
 *
 * Centralises lifecycle, error normalisation, and run-timing so the rest
 * of the app can `await run("MATCH (n) RETURN n")` and get back a
 * structured `RunOutcome` without dealing with promise rejection.
 *
 * MUST be imported from a client component or a `useEffect` — calling
 * `getDb()` on the server throws.
 */

import type {
  Database,
  LoraParams,
  SnapshotInfo,
  SnapshotMeta,
  WasmSnapshotByteOptions,
  WasmSnapshotLoadOptions,
  WasmSnapshotSource,
} from "@loradb/lora-wasm";
import { adapt } from "./adapter";
import type { RunOutcome } from "./types";
import { ulid } from "@/lib/util/id";

let dbPromise: Promise<Database> | null = null;

/**
 * Hard ceiling on how long we'll wait for the WASM database to come
 * up. On most networks the dynamic import + `createDatabase` resolves
 * in well under a second; if we're past this number something's stuck
 * (locked-down browser, blocked WASM origin, networkless cold load)
 * and the workbench should surface that instead of spinning forever.
 */
export const DB_BOOT_TIMEOUT_MS = 15_000;

class DbBootTimeoutError extends Error {
  constructor(timeoutMs: number) {
    super(
      `LoraDB did not finish booting within ${(timeoutMs / 1000).toFixed(0)}s. ` +
        `Check the browser console for WASM/module errors.`,
    );
    this.name = "DbBootTimeoutError";
  }
}

/**
 * Race a promise against a timeout. On timeout, rejects with a
 * {@link DbBootTimeoutError} so callers can pattern-match the failure.
 * The underlying boot promise keeps running — if it later resolves we
 * still cache it, so a "Retry" call after a transient stall succeeds
 * without paying the import cost again.
 */
function withTimeout<T>(p: Promise<T>, timeoutMs: number): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const handle = setTimeout(() => {
      reject(new DbBootTimeoutError(timeoutMs));
    }, timeoutMs);
    p.then(
      (value) => {
        clearTimeout(handle);
        resolve(value);
      },
      (err: unknown) => {
        clearTimeout(handle);
        reject(err);
      },
    );
  });
}

/**
 * Lazily create and memoise the WASM database instance. React StrictMode
 * double-renders are absorbed by the module-scoped promise.
 *
 * Rejects with a `DbBootTimeoutError` after {@link DB_BOOT_TIMEOUT_MS}
 * so the UI can surface a retry button. Throws synchronously on the
 * server so SSR import accidents fail loudly.
 */
export function getDb(): Promise<Database> {
  if (typeof window === "undefined") {
    throw new Error(
      "getDb() called on server — must be invoked inside a client component or effect.",
    );
  }
  if (!dbPromise) {
    dbPromise = (async () => {
      const { createDatabase } = await import("@loradb/lora-wasm");
      return createDatabase({ runtime: "auto" });
    })();
    // If the boot fails (timeout OR creation throw), drop the cached
    // promise so a retry attempt re-imports cleanly.
    dbPromise.catch(() => {
      dbPromise = null;
    });
  }
  return withTimeout(dbPromise, DB_BOOT_TIMEOUT_MS);
}

/** Allow callers to recover from a failed boot without a page reload. */
export function resetDbBoot(): void {
  dbPromise = null;
}

export { DbBootTimeoutError };

// ---------------------------------------------------------------------------
// Error-message position parsing
// ---------------------------------------------------------------------------

// Matches things like "line 3, column 7", "line 3:7", "(3:7)", "at 3:7".
const POSITION_RE = /(?:line\s*)?(\d+)\s*(?:[:,]\s*(?:col(?:umn)?\s*)?|\s+col(?:umn)?\s*)(\d+)/i;

function parsePosition(message: string): { line: number; col: number } | undefined {
  const m = POSITION_RE.exec(message);
  if (!m) return undefined;
  const line = Number(m[1]);
  const col = Number(m[2]);
  if (!Number.isFinite(line) || !Number.isFinite(col)) return undefined;
  return { line, col };
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * Run a query and return a normalised outcome. Never throws: failures are
 * captured as `RunErr` so callers can render them inline.
 */
export async function run(body: string, params?: LoraParams): Promise<RunOutcome> {
  const runId = ulid();
  const startedAt = Date.now();
  try {
    const db = await getDb();
    const raw = await db.execute(body, params);
    const endedAt = Date.now();
    return {
      state: "ok",
      runId,
      startedAt,
      endedAt,
      ms: endedAt - startedAt,
      result: adapt(raw),
    };
  } catch (err) {
    const endedAt = Date.now();
    const message = err instanceof Error ? err.message : String(err);
    const position = parsePosition(message);
    const base = {
      state: "error" as const,
      runId,
      startedAt,
      endedAt,
      ms: endedAt - startedAt,
      message,
    };
    return position ? { ...base, position } : base;
  }
}

/** Drop all nodes/relationships from the database. */
export async function reset(): Promise<void> {
  const db = await getDb();
  await db.clear();
}

/**
 * Serialize the current graph. Always returns the raw `Uint8Array` byte form
 * so callers can stash it in IndexedDB without an extra conversion step.
 * Optional compression/encryption are forwarded; the format is fixed.
 */
export async function saveSnapshot(
  opts?: Omit<WasmSnapshotByteOptions, "format">,
): Promise<Uint8Array> {
  const db = await getDb();
  return db.saveSnapshot({ format: "bytes", ...(opts ?? {}) });
}

/** Restore the database state from a snapshot source (bytes/Blob/Response/URL/…). */
export async function loadSnapshot(
  source: WasmSnapshotSource,
  opts?: WasmSnapshotLoadOptions,
): Promise<SnapshotMeta> {
  const db = await getDb();
  return db.loadSnapshot(source, opts);
}

/**
 * Inspect snapshot header metadata from raw bytes. Routes through the same
 * WASM module that backs the live database — so in the browser this is a
 * worker round-trip, in Node it's an in-process call.
 */
export async function readSnapshotInfo(
  bytes: Uint8Array,
): Promise<SnapshotInfo> {
  const db = await getDb();
  return db.snapshotInfo(bytes);
}

/** Current count of nodes in the database. */
export async function nodeCount(): Promise<number> {
  const db = await getDb();
  return db.nodeCount();
}

/** Current count of relationships in the database. */
export async function relationshipCount(): Promise<number> {
  const db = await getDb();
  return db.relationshipCount();
}
