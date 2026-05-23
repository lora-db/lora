/**
 * Non-blocking worker architecture: verified against an in-process stub that
 * speaks the same message protocol as the real worker. This proves the wire
 * format and the client's promise correlation; spawning a real Web Worker
 * requires a browser host, which we demo in `examples/browser.html`.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import {
  createDatabase,
  createWorkerDatabase,
  LoraError,
  type Database,
  type WorkerDatabase,
  type WorkerLike,
} from "../ts/index.js";
import type { Request, Response } from "../ts/worker-protocol.js";
import type { LoraErrorCode } from "../ts/types.js";

class InProcessWorker {
  #listeners: {
    message: Array<(e: { data: Response }) => void>;
    error: Array<(e: { message?: string }) => void>;
    messageerror: Array<(e: { message?: string }) => void>;
  } = { message: [], error: [], messageerror: [] };
  #db: Database | null = null;
  #nextStreamId = 1;
  #streams = new Map<
    number,
    AsyncIterableIterator<Record<string, unknown>> & {
      columns?: () => string[] | Promise<string[]>;
      close?: () => void;
    }
  >();

  addEventListener(
    type: "message" | "error" | "messageerror",
    listener:
      | ((e: { data: Response }) => void)
      | ((e: { message?: string }) => void),
  ): void {
    if (type === "message") {
      this.#listeners.message.push(listener as (e: { data: Response }) => void);
    } else if (type === "error") {
      this.#listeners.error.push(listener as (e: { message?: string }) => void);
    } else {
      this.#listeners.messageerror.push(
        listener as (e: { message?: string }) => void,
      );
    }
  }

  removeEventListener(
    type: "message",
    listener: (e: { data: Response }) => void,
  ): void {
    this.#listeners.message = this.#listeners.message.filter(
      (l) => l !== listener,
    );
    void type;
  }

  terminate(): void {
    this.#db?.dispose();
    this.#db = null;
  }

  postMessage(msg: unknown): void {
    void this.#handle(msg as Request);
  }

  async #handle(msg: Request): Promise<void> {
    const respond = (body: Response["body"]) => {
      const response: Response = { id: msg.id, body };
      queueMicrotask(() => {
        for (const l of this.#listeners.message) l({ data: response });
      });
    };

    try {
      if (!this.#db) this.#db = await createDatabase();
      const db = this.#db;

      switch (msg.body.op) {
        case "execute": {
          const result = await db.execute(
            msg.body.query,
            msg.body.params ?? undefined,
          );
          respond({ ok: true, result });
          break;
        }
        case "transaction": {
          const result = await db.transaction(
            msg.body.statements,
            msg.body.mode ?? "read_write",
          );
          respond({ ok: true, result });
          break;
        }
        case "streamOpen": {
          const stream = db.stream(
            msg.body.query,
            msg.body.params ?? undefined,
          );
          const streamId = this.#nextStreamId++;
          this.#streams.set(streamId, stream);
          respond({
            ok: true,
            result: { streamId, columns: await stream.columns() },
          });
          break;
        }
        case "streamNext": {
          const stream = this.#streams.get(msg.body.streamId);
          if (!stream) throw new Error("LORA_INTERNAL: query stream is closed");
          const next = await stream.next();
          if (next.done) {
            this.#streams.delete(msg.body.streamId);
            respond({ ok: true, result: null });
          } else {
            respond({ ok: true, result: next.value });
          }
          break;
        }
        case "streamClose": {
          const stream = this.#streams.get(msg.body.streamId);
          stream?.close?.();
          this.#streams.delete(msg.body.streamId);
          respond({ ok: true, result: null });
          break;
        }
        case "clear": {
          await db.clear();
          respond({ ok: true, result: null });
          break;
        }
        case "nodeCount": {
          respond({ ok: true, result: await db.nodeCount() });
          break;
        }
        case "relationshipCount": {
          respond({ ok: true, result: await db.relationshipCount() });
          break;
        }
        case "dispose": {
          for (const stream of this.#streams.values()) stream.close?.();
          this.#streams.clear();
          db.dispose();
          this.#db = null;
          respond({ ok: true, result: null });
          break;
        }
      }
    } catch (err) {
      // Preserve LoraError.code when Database.execute already narrowed it.
      if (err instanceof LoraError) {
        respond({ ok: false, error: { message: err.message, code: err.code } });
        return;
      }
      const message = err instanceof Error ? err.message : String(err);
      const match = /^(LORA_[A-Z_]+|WORKER_ERROR):\s*(.*)$/s.exec(message);
      const code: LoraErrorCode =
        (match?.[1] as LoraErrorCode | undefined) ?? "UNKNOWN";
      const cleanedMessage = match ? match[2]! : message;
      respond({ ok: false, error: { message: cleanedMessage, code } });
    }
  }
}

describe("WorkerDatabase — message protocol", () => {
  let worker: InProcessWorker;
  let db: WorkerDatabase;

  beforeEach(() => {
    worker = new InProcessWorker();
    db = createWorkerDatabase(worker);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("creates a node and counts it over the worker protocol", async () => {
    await db.execute("CREATE (:X {n: 1})");
    expect(await db.nodeCount()).toBe(1);
  });

  it("returns typed rows through the message boundary", async () => {
    await db.execute("CREATE (:P {name: $n})", { n: "Bob" });
    const result = await db.execute<{ name: string }>(
      "MATCH (n:P) RETURN n.name AS name",
    );
    expect(result.rows[0]!.name).toBe("Bob");
  });

  it("supports stream and transaction helpers over the worker protocol", async () => {
    await db.transaction([
      { query: "UNWIND list.range(1, 3) AS i CREATE (:W {i: i})" },
    ]);

    const seen: number[] = [];
    for await (const row of db.stream<{ i: number }>(
      "MATCH (n:W) RETURN n.i AS i ORDER BY i",
    )) {
      seen.push(row.i);
    }
    expect(seen).toEqual([1, 2, 3]);
  });

  it("surfaces LORA_PARSE from the worker", async () => {
    await expect(db.execute("NOT CYPHER")).rejects.toSatisfy(
      (e) =>
        e instanceof Error &&
        (e as { code?: string }).code === "LORA_PARSE" &&
        !e.message.startsWith("LORA_PARSE:"),
    );
  });

  it("rejects and clears a request when postMessage throws", async () => {
    const throwingDb = createWorkerDatabase({
      addEventListener() {
        // no-op
      },
      removeEventListener() {
        // no-op
      },
      postMessage() {
        throw new Error("post failed");
      },
      terminate() {
        // no-op
      },
    } as never);

    await expect(throwingDb.nodeCount()).rejects.toThrow("post failed");
  });

  it("handles many concurrent queries without deadlock", async () => {
    await db.execute("CREATE (:Counter {n: 0})");
    const results = await Promise.all(
      Array.from({ length: 20 }, (_, i) =>
        db.execute<{ v: number }>("RETURN $v AS v", { v: i }),
      ),
    );
    expect(results.map((r) => r.rows[0]!.v)).toEqual(
      Array.from({ length: 20 }, (_, i) => i),
    );
  });

  it("rejects a worker request that never receives a response", async () => {
    vi.useFakeTimers();
    const silent = new ManualWorker({ respond: false });
    const stalledDb = createWorkerDatabase(silent, { requestTimeoutMs: 5 });

    const promise = stalledDb.nodeCount();
    const assertion = expect(promise).rejects.toSatisfy(
      (e) =>
        e instanceof LoraError &&
        e.code === "WORKER_ERROR" &&
        /did not respond/.test(e.message),
    );
    await vi.advanceTimersByTimeAsync(5);

    await assertion;
  });

  it("rejects future calls after a worker error", async () => {
    const manual = new ManualWorker({ respond: false });
    const failedDb = createWorkerDatabase(manual, { requestTimeoutMs: 0 });

    const pending = failedDb.nodeCount();
    manual.emitError("boom");

    await expect(pending).rejects.toSatisfy(
      (e) => e instanceof LoraError && e.code === "WORKER_ERROR",
    );
    await expect(failedDb.nodeCount()).rejects.toSatisfy(
      (e) =>
        e instanceof LoraError &&
        e.code === "WORKER_ERROR" &&
        e.message === "boom",
    );
  });

  it("rejects malformed worker responses instead of hanging", async () => {
    const manual = new ManualWorker({ respond: false });
    const malformedDb = createWorkerDatabase(manual, { requestTimeoutMs: 0 });

    const pending = malformedDb.nodeCount();
    manual.emitMessage({
      id: 1,
      body: { ok: false, error: { message: "bad", code: "LORA_FUTURE" } },
    });

    await expect(pending).rejects.toSatisfy(
      (e) =>
        e instanceof LoraError &&
        e.code === "WORKER_ERROR" &&
        /malformed/.test(e.message),
    );
  });

  it("wraps synchronous postMessage failures as worker errors", async () => {
    const throwing = new ManualWorker({
      respond: false,
      postMessageError: new Error("clone failed"),
    });
    const throwingDb = createWorkerDatabase(throwing, { requestTimeoutMs: 0 });

    await expect(throwingDb.nodeCount()).rejects.toSatisfy(
      (e) =>
        e instanceof LoraError &&
        e.code === "WORKER_ERROR" &&
        e.message === "clone failed",
    );
  });

  it("rejects calls after dispose closes the client", async () => {
    const manual = new ManualWorker({ respond: true });
    const disposedDb = createWorkerDatabase(manual, { requestTimeoutMs: 0 });

    await disposedDb.dispose();

    await expect(disposedDb.nodeCount()).rejects.toSatisfy(
      (e) =>
        e instanceof LoraError &&
        e.code === "WORKER_ERROR" &&
        e.message === "database worker is closed",
    );
  });
});

class ManualWorker implements WorkerLike {
  readonly #listeners: {
    message: Array<(e: { data: unknown }) => void>;
    error: Array<(e: { message?: string }) => void>;
    messageerror: Array<(e: { message?: string }) => void>;
  } = { message: [], error: [], messageerror: [] };

  constructor(
    private readonly options: {
      respond: boolean;
      postMessageError?: Error;
    },
  ) {}

  addEventListener(
    type: "message" | "error" | "messageerror",
    listener:
      | ((e: { data: unknown }) => void)
      | ((e: { message?: string }) => void),
  ): void {
    if (type === "message") {
      this.#listeners.message.push(listener as (e: { data: unknown }) => void);
    } else if (type === "error") {
      this.#listeners.error.push(listener as (e: { message?: string }) => void);
    } else {
      this.#listeners.messageerror.push(
        listener as (e: { message?: string }) => void,
      );
    }
  }

  removeEventListener(
    type: "message",
    listener: (e: { data: unknown }) => void,
  ): void {
    this.#listeners.message = this.#listeners.message.filter(
      (l) => l !== listener,
    );
    void type;
  }

  postMessage(message: unknown): void {
    if (this.options.postMessageError) throw this.options.postMessageError;
    if (!this.options.respond) return;
    const request = message as Request;
    this.emitMessage({ id: request.id, body: { ok: true, result: null } });
  }

  terminate(): void {
    // no-op
  }

  emitMessage(data: unknown): void {
    for (const listener of this.#listeners.message) listener({ data });
  }

  emitError(message: string): void {
    for (const listener of this.#listeners.error) listener({ message });
  }
}
