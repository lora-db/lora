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
} from "../pkg-web/lora_wasm.js";
import type { Request, Response } from "./worker-protocol.js";
import type { LoraErrorCode } from "./types.js";

declare const self: DedicatedWorkerGlobalScope;

let db: WasmDatabase | null = null;
let ready: Promise<void> | null = null;
let nextStreamId = 1;
const streams = new Map<number, {
  columns(): unknown;
  next(): unknown;
  close(): void;
}>();

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

function extractErrorCode(message: string): LoraErrorCode {
  const match = /^(LORA_ERROR|INVALID_PARAMS|WORKER_ERROR):/.exec(message);
  return (match?.[1] as LoraErrorCode | undefined) ?? "UNKNOWN";
}

self.onmessage = async (event: MessageEvent<Request>) => {
  const { id, body } = event.data;
  const respond = (payload: Response["body"]) => {
    const res: Response = { id, body: payload };
    self.postMessage(res);
  };

  try {
    await ensureReady();
    if (!db) throw new Error("WORKER_ERROR: database not initialized");

    switch (body.op) {
      case "execute": {
        const result = db.execute(body.query, body.params ?? null) as unknown as Response["body"];
        respond({ ok: true, result: result as never });
        break;
      }
      case "streamOpen": {
        const native = db as unknown as {
          openStream(query: string, params: unknown): {
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
        if (!stream) throw new Error("LORA_ERROR: query stream is closed");
        const row = stream.next();
        if (row === null) {
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
        const result = native.transaction(body.statements, body.mode ?? "read_write");
        respond({ ok: true, result: result as never });
        break;
      }
      case "saveSnapshotToBytes": {
        const native = db as unknown as {
          saveSnapshotToBytes(): Uint8Array;
        };
        respond({ ok: true, result: native.saveSnapshotToBytes() });
        break;
      }
      case "loadSnapshotFromBytes": {
        const native = db as unknown as {
          loadSnapshotFromBytes(bytes: Uint8Array): unknown;
        };
        respond({ ok: true, result: native.loadSnapshotFromBytes(body.bytes) as never });
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
      case "dispose": {
        for (const stream of streams.values()) stream.close();
        streams.clear();
        db.free();
        db = null;
        respond({ ok: true, result: null });
        break;
      }
    }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    respond({
      ok: false,
      error: { message, code: extractErrorCode(message) },
    });
  }
};
