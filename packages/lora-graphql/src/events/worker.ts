// `lora.worker()`: the side effects of an `@outbox` type (a push, an
// e-mail, a webhook), after the commit. Each pending node is claimed with
// one write, handed to the handler, and marked done or scheduled again.
// Delivery is at least once: a handler may run twice for one node (a
// crash after the side effect, before the node is marked), so it must be
// idempotent; the node's key is its idempotency key.

import { name } from "../compile/cypher.js";
import type { LoraDriver } from "../driver.js";
import type { WriteChange } from "../execute/changes.js";
import type { NodeType } from "../model/types.js";
import type { DatabaseErrorEvent } from "../options.js";
import { OUTBOX } from "./outbox.js";

export interface WorkerOptions {
  /**
   * Called with the node's stored scalar fields, by field name, read
   * without read rules. Resolving marks the node done; throwing (or
   * rejecting) schedules it again, up to `maxAttempts`.
   */
  handle: (
    node: Record<string, unknown>,
    info: {
      /** The node's @key value: the idempotency key of the work. */
      key: unknown;
      /** 1 for the first call, 2 for the first retry, ... */
      attempt: number;
      /** Aborted when the worker stops. */
      signal: AbortSignal;
    },
  ) => unknown;
  /** Handlers running at once. Default 4. */
  concurrency?: number;
  /**
   * Calls before a node is given up as `FAILED` and reported to
   * `onError`. Default 8.
   */
  maxAttempts?: number;
  /**
   * Milliseconds to wait before attempt `attempt + 1`. Default: one
   * second doubling per attempt, at most five minutes.
   */
  backoffMs?: (attempt: number) => number;
  /**
   * How often the worker looks for pending nodes it was not told about:
   * work left by a restart, retries that are due, claims whose worker
   * died. Milliseconds, default 30 000.
   */
  sweepMs?: number;
  /**
   * How long a claim holds, in milliseconds. A handler that takes longer
   * may see its node handed out again. Default 60 000.
   */
  leaseMs?: number;
  /** What to do with a node once handled. Default `"keep"`. */
  onDone?: "keep" | "delete";
}

export interface Worker {
  /**
   * Resolves once nothing is pending: after a look at the database, with
   * no handler running and no node due. For tests and for shutdown.
   */
  idle(): Promise<void>;
  /** Stop taking work; resolves when the running handlers have ended. */
  stop(): Promise<void>;
}

export interface WorkerHost {
  driver: LoraDriver;
  node: NodeType;
  timeoutMs: number;
  /** Committed writes, from the change hub. */
  listen: (listener: (change: WriteChange) => void) => () => unknown;
  /** Report a deleted node to the change hub (`onDone: "delete"`). */
  emit: (change: WriteChange) => void;
  onError: ((event: DatabaseErrorEvent) => void) | undefined;
}

/** Most nodes one sweep takes on. */
const SWEEP_LIMIT = 1000;

export class OutboxWorker implements Worker {
  readonly #host: WorkerHost;
  readonly #options: Required<Omit<WorkerOptions, "handle">> &
    Pick<WorkerOptions, "handle">;
  readonly #label: string;
  readonly #key: string;
  readonly #queue = new Map<string, unknown>();
  readonly #abort = new AbortController();
  readonly #unlisten: () => unknown;
  readonly #timer: ReturnType<typeof setInterval>;
  #running = 0;
  #sweeping: Promise<void> | undefined;
  #waiters: Array<() => void> = [];

  constructor(host: WorkerHost, options: WorkerOptions) {
    this.#host = host;
    this.#options = {
      concurrency: 4,
      maxAttempts: 8,
      backoffMs: (attempt) => Math.min(1000 * 2 ** (attempt - 1), 300_000),
      sweepMs: 30_000,
      leaseMs: 60_000,
      onDone: "keep",
      ...options,
    };
    this.#label = name(host.node.labels[0]!);
    this.#key = name(host.node.key.property);
    this.#unlisten = host.listen((change) => {
      if (this.#abort.signal.aborted) return;
      // A broad change may have created anything: look.
      if (change.broad) return void this.#sweep();
      for (const ref of change.created) {
        if (ref.type === host.node.name) this.#enqueue(ref.key);
      }
    });
    this.#timer = setInterval(() => void this.#sweep(), this.#options.sweepMs);
    // A waiting worker does not keep the process alive.
    (this.#timer as { unref?: () => void }).unref?.();
    void this.#sweep();
  }

  async idle(): Promise<void> {
    for (;;) {
      await this.#sweep();
      if (this.#queue.size === 0 && this.#running === 0) return;
      await new Promise<void>((resolve) => this.#waiters.push(resolve));
    }
  }

  async stop(): Promise<void> {
    this.#abort.abort();
    this.#unlisten();
    clearInterval(this.#timer);
    this.#queue.clear();
    while (this.#running > 0) {
      await new Promise<void>((resolve) => this.#waiters.push(resolve));
    }
  }

  /** Take on the nodes that are due: pending ones, and dead claims. */
  #sweep(): Promise<void> {
    if (this.#abort.signal.aborted) return Promise.resolve();
    this.#sweeping ??= (async () => {
      try {
        const [result] = await this.#host.driver.run(
          [
            {
              text:
                `MATCH (n:${this.#label}) WHERE ${this.#due("n")}\n` +
                `RETURN n.${this.#key} AS key LIMIT ${SWEEP_LIMIT}`,
              params: { now: Date.now() },
            },
          ],
          { mode: "read", timeoutMs: this.#host.timeoutMs },
        );
        for (const row of result!.rows) this.#enqueue(row["key"]);
      } catch (err) {
        this.#report(err);
      } finally {
        this.#sweeping = undefined;
      }
    })();
    return this.#sweeping;
  }

  /** Pending and due, or claimed by a worker whose lease ran out. */
  #due(n: string): string {
    return (
      `((${n}.${OUTBOX.state} = "PENDING" AND coalesce(${n}.${OUTBOX.at}, 0) <= $now)` +
      ` OR (${n}.${OUTBOX.state} = "CLAIMED" AND ${n}.${OUTBOX.lease} <= $now))`
    );
  }

  #enqueue(key: unknown): void {
    if (this.#abort.signal.aborted) return;
    this.#queue.set(JSON.stringify(key), key);
    this.#pump();
  }

  #pump(): void {
    while (this.#running < this.#options.concurrency && this.#queue.size > 0) {
      const [id, key] = this.#queue.entries().next().value!;
      this.#queue.delete(id);
      this.#running++;
      void this.#process(key)
        .catch((err: unknown) => this.#report(err))
        .finally(() => {
          this.#running--;
          this.#pump();
          for (const wake of this.#waiters.splice(0)) wake();
        });
    }
  }

  async #process(key: unknown): Promise<void> {
    const now = Date.now();
    // One write claims the node: with one writer at a time, two workers
    // on one database never both get a row back.
    const claimed = await this.#write(
      `MATCH (n:${this.#label}) WHERE n.${this.#key} = $key AND ${this.#due("n")}\n` +
        `SET n.${OUTBOX.state} = "CLAIMED", n.${OUTBOX.lease} = $until\n` +
        `RETURN properties(n) AS props`,
      { key, now, until: now + this.#options.leaseMs },
    );
    const props = claimed[0]?.["props"] as Record<string, unknown> | undefined;
    if (!props) return;
    const attempt = Number(props[OUTBOX.attempts] ?? 0) + 1;
    const node: Record<string, unknown> = {};
    for (const f of this.#host.node.fields.values()) {
      if (f.kind === "scalar") node[f.name] = props[f.property] ?? null;
    }
    try {
      await this.#options.handle(node, {
        key,
        attempt,
        signal: this.#abort.signal,
      });
    } catch (err) {
      const failed = attempt >= this.#options.maxAttempts;
      await this.#write(
        `MATCH (n:${this.#label}) WHERE n.${this.#key} = $key AND n.${OUTBOX.state} = "CLAIMED"\n` +
          `SET n.${OUTBOX.state} = $state, n.${OUTBOX.attempts} = $attempt, n.${OUTBOX.at} = $at, n.${OUTBOX.error} = $error\n` +
          `REMOVE n.${OUTBOX.lease}`,
        {
          key,
          state: failed ? "FAILED" : "PENDING",
          attempt,
          at: Date.now() + this.#options.backoffMs(attempt),
          error: (err instanceof Error ? err.message : String(err)).slice(
            0,
            1000,
          ),
        },
      );
      if (failed) this.#report(err);
      return;
    }
    if (this.#options.onDone === "delete") {
      await this.#write(
        `MATCH (n:${this.#label}) WHERE n.${this.#key} = $key AND n.${OUTBOX.state} = "CLAIMED"\nDETACH DELETE n`,
        { key },
      );
      const ref = { type: this.#host.node.name, key };
      this.#host.emit({
        operation: "DELETE",
        field: "worker",
        created: [],
        updated: [],
        deleted: [ref],
        connected: [],
        disconnected: [],
        entities: [ref],
        types: [ref.type],
        relationshipTypes: [],
        broad: false,
      });
    } else {
      await this.#write(
        `MATCH (n:${this.#label}) WHERE n.${this.#key} = $key AND n.${OUTBOX.state} = "CLAIMED"\n` +
          `SET n.${OUTBOX.state} = "DONE"\n` +
          `REMOVE n.${OUTBOX.lease}, n.${OUTBOX.at}, n.${OUTBOX.error}`,
        { key },
      );
    }
  }

  async #write(
    text: string,
    params: Record<string, unknown>,
  ): Promise<Array<Record<string, unknown>>> {
    const [result] = await this.#host.driver.run([{ text, params }], {
      mode: "write",
      timeoutMs: this.#host.timeoutMs,
    });
    return result!.rows;
  }

  #report(err: unknown): void {
    try {
      this.#host.onError?.({
        id: globalThis.crypto.randomUUID(),
        field: `worker:${this.#host.node.name}`,
        message: err instanceof Error ? err.message : String(err),
        error: err,
      });
    } catch {
      // A failing error hook must not stop the worker.
    }
  }
}
