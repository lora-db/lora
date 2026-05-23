/**
 * Worker entrypoint — hosts the WASM engine inside a Web Worker so the main
 * thread never runs heavy query work.
 *
 * This file uses the `--target web` wasm-pack output (`pkg-web/`) which
 * takes care of fetching and instantiating the `.wasm` binary itself via
 * the URL passed to `__wbg_init`. That avoids a hard dependency on a
 * bundler plugin for WASM and lets this file run unchanged in any module
 * worker (Vite dev, a static file server, a built tarball).
 */

/// <reference lib="webworker" />

import __wbg_init, {
  WasmDatabase,
  init as installPanicHook,
  snapshotInfo as wasmSnapshotInfo,
} from "../pkg-web/lora_wasm.js";
import type { Request, Response } from "./worker-protocol.js";
import { isLoraErrorCode, type LoraErrorCode } from "./types.js";

declare const self: DedicatedWorkerGlobalScope;

let db: WasmDatabase | null = null;
let ready: Promise<void> | null = null;
let nextStreamId = 1;
const streams = new Map<
  number,
  {
    columns(): unknown;
    next(): unknown;
    close(): void;
  }
>();

interface NativeExport {
  columns(): unknown;
  next(): unknown;
  close(): void;
}

let nextExportId = 1;
const exports_ = new Map<number, NativeExport>();

interface NativeImport {
  feed(chunk: Uint8Array): unknown;
  finish(): unknown;
  close(): void;
}

let nextImportId = 1;
const imports_ = new Map<number, NativeImport>();

function ensureReady(): Promise<void> {
  if (!ready) {
    ready = (async () => {
      const wasmUrl = new URL("../pkg-web/lora_wasm_bg.wasm", import.meta.url);
      await __wbg_init(wasmUrl);
      installPanicHook();
      db = new WasmDatabase();
    })();
  }
  return ready;
}

function normalizeError(message: string): {
  code: LoraErrorCode;
  message: string;
} {
  const match = /^(LORA_[A-Z_]+|WORKER_ERROR):\s*(.*)$/s.exec(message);
  return {
    code: match && isLoraErrorCode(match[1]) ? match[1] : "UNKNOWN",
    message: match ? match[2]! : message,
  };
}

self.onmessage = async (event: MessageEvent<Request>) => {
  const { id, body } = event.data;
  const respond = (payload: Response["body"], transfer?: Transferable[]) => {
    const res: Response = { id, body: payload };
    self.postMessage(res, transfer ?? []);
  };

  try {
    await ensureReady();
    if (!db) throw new Error("WORKER_ERROR: database not initialized");

    switch (body.op) {
      case "execute": {
        // executeBuffer returns the encoded result as a Uint8Array;
        // its underlying ArrayBuffer is transferable, so postMessage
        // skips structured-clone entirely. The client decodes on the
        // main thread in JIT'd JavaScript.
        const native = db as unknown as {
          executeBuffer(query: string, params: unknown): Uint8Array;
        };
        const buf = native.executeBuffer(body.query, body.params ?? null);
        respond({ ok: true, result: buf as never }, [buf.buffer]);
        break;
      }
      case "streamOpen": {
        const native = db as unknown as {
          openStream(
            query: string,
            params: unknown,
          ): {
            columns(): unknown;
            next(): unknown;
            close(): void;
          };
        };
        const stream = native.openStream(body.query, body.params ?? null);
        const streamId = nextStreamId++;
        streams.set(streamId, stream);
        respond({
          ok: true,
          result: { streamId, columns: stream.columns() as string[] },
        });
        break;
      }
      case "streamNext": {
        const stream = streams.get(body.streamId);
        if (!stream) throw new Error("LORA_INTERNAL: query stream is closed");
        const row = stream.next();
        if (row === null) {
          stream.close();
          streams.delete(body.streamId);
        }
        respond({ ok: true, result: row as never });
        break;
      }
      case "streamClose": {
        const stream = streams.get(body.streamId);
        if (stream) {
          stream.close();
          streams.delete(body.streamId);
        }
        respond({ ok: true, result: null });
        break;
      }
      case "transaction": {
        const native = db as unknown as {
          transaction(statements: unknown, mode: string): unknown;
        };
        const result = native.transaction(
          body.statements,
          body.mode ?? "read_write",
        );
        respond({ ok: true, result: result as never });
        break;
      }
      case "saveSnapshot": {
        const native = db as unknown as {
          saveSnapshot(options?: unknown): Uint8Array;
        };
        respond({
          ok: true,
          result: native.saveSnapshot(body.options ?? null),
        });
        break;
      }
      case "loadSnapshot": {
        const native = db as unknown as {
          loadSnapshot(bytes: Uint8Array, options?: unknown): unknown;
        };
        respond({
          ok: true,
          result: native.loadSnapshot(
            body.bytes,
            body.options ?? null,
          ) as never,
        });
        break;
      }
      case "snapshotInfo": {
        respond({
          ok: true,
          result: wasmSnapshotInfo(body.bytes) as never,
        });
        break;
      }
      case "exportRows": {
        const native = db as unknown as {
          exportRows(
            query: string,
            params: unknown,
            format: string,
          ): { bytes: Uint8Array; stats: { rows: number } };
        };
        const out = native.exportRows(
          body.query,
          body.params ?? null,
          body.format,
        );
        // The bytes buffer is transferable — hand it over so the
        // postMessage skips structured-clone of potentially huge payloads.
        respond({ ok: true, result: out as never }, [out.bytes.buffer]);
        break;
      }
      case "exportOpen": {
        // Open a streaming export cursor. Each subsequent `exportNext`
        // call pulls one chunk's worth of rows from the engine and
        // ships back the encoded bytes. No full-result materialization
        // happens at open time — only the engine's pull cursor is set
        // up. Mirrors `streamOpen` for query rows.
        const native = db as unknown as {
          openExport(
            query: string,
            params: unknown,
            format: string,
          ): NativeExport;
        };
        const cursor = native.openExport(
          body.query,
          body.params ?? null,
          body.format,
        );
        const exportId = nextExportId++;
        exports_.set(exportId, cursor);
        respond({
          ok: true,
          result: { exportId, columns: cursor.columns() as string[] },
        });
        break;
      }
      case "exportNext": {
        const cursor = exports_.get(body.exportId);
        if (!cursor) throw new Error("LORA_INTERNAL: export cursor is closed");
        const chunk = cursor.next() as Uint8Array | null;
        if (chunk === null) {
          cursor.close();
          exports_.delete(body.exportId);
          respond({ ok: true, result: null });
        } else {
          // Transfer the underlying ArrayBuffer so the chunk hops
          // worker→main without a structured-clone copy.
          respond({ ok: true, result: chunk as never }, [
            chunk.buffer as ArrayBuffer,
          ]);
        }
        break;
      }
      case "exportClose": {
        const cursor = exports_.get(body.exportId);
        if (cursor) {
          cursor.close();
          exports_.delete(body.exportId);
        }
        respond({ ok: true, result: null });
        break;
      }
      case "importOpen": {
        const native = db as unknown as {
          openImport(
            format: string,
            mappingOrTemplate: unknown,
            batchSize: number | null,
            dryRun: boolean | null,
            permissive: boolean | null,
          ): NativeImport;
        };
        const cursor = native.openImport(
          body.format,
          body.mappingOrTemplate,
          body.batchSize ?? null,
          body.dryRun ?? null,
          body.permissive ?? null,
        );
        const importId = nextImportId++;
        imports_.set(importId, cursor);
        respond({ ok: true, result: { importId } });
        break;
      }
      case "importFeed": {
        const cursor = imports_.get(body.importId);
        if (!cursor) throw new Error("LORA_INTERNAL: import cursor is closed");
        const progress = cursor.feed(body.chunk);
        respond({ ok: true, result: progress as never });
        break;
      }
      case "importFinish": {
        const cursor = imports_.get(body.importId);
        if (!cursor) throw new Error("LORA_INTERNAL: import cursor is closed");
        try {
          const stats = cursor.finish();
          respond({ ok: true, result: stats as never });
        } finally {
          cursor.close();
          imports_.delete(body.importId);
        }
        break;
      }
      case "importClose": {
        const cursor = imports_.get(body.importId);
        if (cursor) {
          cursor.close();
          imports_.delete(body.importId);
        }
        respond({ ok: true, result: null });
        break;
      }
      case "importRows": {
        const native = db as unknown as {
          importRows(
            bytes: Uint8Array,
            format: string,
            mapping: unknown,
            batchSize?: number | null,
          ): { rows: number; batches: number };
        };
        respond({
          ok: true,
          result: native.importRows(
            body.bytes,
            body.format,
            body.mapping,
            body.batchSize ?? null,
          ) as never,
        });
        break;
      }
      case "importRowsWithCypher": {
        const native = db as unknown as {
          importRowsWithCypher(
            bytes: Uint8Array,
            format: string,
            template: string,
            batchSize?: number | null,
          ): { rows: number; batches: number };
        };
        respond({
          ok: true,
          result: native.importRowsWithCypher(
            body.bytes,
            body.format,
            body.template,
            body.batchSize ?? null,
          ) as never,
        });
        break;
      }
      case "clear": {
        db.clear();
        respond({ ok: true, result: null });
        break;
      }
      case "nodeCount": {
        respond({ ok: true, result: db.nodeCount() });
        break;
      }
      case "relationshipCount": {
        respond({ ok: true, result: db.relationshipCount() });
        break;
      }
      case "graphStats": {
        respond({
          ok: true,
          result: db.graphStats() as Record<string, unknown>,
        });
        break;
      }
      case "memoryReport": {
        respond({
          ok: true,
          result: db.memoryReport() as Record<string, unknown>,
        });
        break;
      }
      case "dispose": {
        for (const stream of streams.values()) stream.close();
        streams.clear();
        for (const cursor of exports_.values()) cursor.close();
        exports_.clear();
        for (const cursor of imports_.values()) cursor.close();
        imports_.clear();
        db.free();
        db = null;
        respond({ ok: true, result: null });
        break;
      }
    }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    const error = normalizeError(message);
    respond({
      ok: false,
      error,
    });
  }
};
