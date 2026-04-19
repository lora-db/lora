/**
 * Non-blocking worker architecture: verified against an in-process stub that
 * speaks the same message protocol as the real worker. This proves the wire
 * format and the client's promise correlation; spawning a real Web Worker
 * requires a browser host, which we demo in `examples/browser.html`.
 */

import { describe, it, expect, beforeEach } from "vitest";
import { Database, createWorkerDatabase, LoraError, type WorkerDatabase } from "../ts/index.js";
import type { Request, Response } from "../ts/worker-protocol.js";
import type { LoraErrorCode } from "../ts/types.js";

class InProcessWorker {
  #listeners: {
    message: Array<(e: { data: Response }) => void>;
    error: Array<(e: { message?: string }) => void>;
  } = { message: [], error: [] };
  #db: Database | null = null;

  addEventListener(
    type: "message" | "error",
    listener: ((e: { data: Response }) => void) | ((e: { message?: string }) => void),
  ): void {
    if (type === "message") {
      this.#listeners.message.push(listener as (e: { data: Response }) => void);
    } else {
      this.#listeners.error.push(listener as (e: { message?: string }) => void);
    }
  }

  removeEventListener(
    type: "message",
    listener: (e: { data: Response }) => void,
  ): void {
    this.#listeners.message = this.#listeners.message.filter((l) => l !== listener);
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
      if (!this.#db) this.#db = await Database.create();
      const db = this.#db;

      switch (msg.body.op) {
        case "execute": {
          const result = await db.execute(msg.body.query, msg.body.params ?? undefined);
          respond({ ok: true, result });
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
      const match = /^(LORA_ERROR|INVALID_PARAMS|WORKER_ERROR):\s*(.*)$/s.exec(message);
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

  it("surfaces LORA_ERROR from the worker", async () => {
    await expect(db.execute("NOT CYPHER")).rejects.toSatisfy(
      (e) => e instanceof Error && (e as { code?: string }).code === "LORA_ERROR",
    );
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
});
