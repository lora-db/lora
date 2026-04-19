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
