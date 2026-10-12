// The narrow interface the library needs from a LoraDB database, plus one
// adapter that fits both `@loradb/lora-node` and `@loradb/lora-wasm`
// structurally, so neither binding is a dependency of this package.

export type LoraParams = Record<string, unknown>;

export interface Statement {
  text: string;
  params: LoraParams;
}

export interface QueryResult {
  columns: string[];
  rows: Array<Record<string, unknown>>;
}

export interface PlanNode {
  id: number;
  operator: string;
  details: Record<string, string>;
  estimatedRows: number | null;
  children: PlanNode[];
}

export interface QueryPlan {
  query: string;
  shape: "readOnly" | "mutating";
  resultColumns: string[];
  tree: PlanNode;
}

export interface RunOptions {
  /** `read` runs in a read-only transaction: the engine rejects writes. */
  mode: "read" | "write";
  timeoutMs?: number | undefined;
  signal?: AbortSignal | undefined;
  /**
   * A read the library vouches for: generated, or a @cypher statement
   * checked read-only at startup. The driver may run it outside a
   * read-only transaction.
   */
  verified?: boolean;
  /**
   * A verified read of at most one row, by an exact @key seek, projecting
   * only stored properties. Cheap enough to run synchronously.
   */
  bounded?: boolean;
  /**
   * A mutation sent as one write batch: counted against
   * `maxQueuedWrites`, as a transaction waiting for the lock is.
   */
  queued?: boolean;
}

/** An open interactive transaction: statements see earlier writes. */
export interface DriverTransaction {
  execute(statement: Statement): Promise<QueryResult>;
  /**
   * Run several statements in one call, in order, each seeing the writes
   * of those before it. The first failure rolls the transaction back and
   * rejects with that statement's error. Optional: without it the
   * statements run one `execute()` at a time.
   */
  executeMany?(statements: Statement[]): Promise<QueryResult[]>;
  commit(): Promise<void>;
  rollback(): Promise<void>;
  /** False once committed, rolled back, or failed (a failure rolls back). */
  readonly isOpen: boolean;
}

export interface LoraDriver {
  /** Run statements atomically, in one transaction, in order. */
  run(statements: Statement[], options: RunOptions): Promise<QueryResult[]>;
  /**
   * Open an interactive transaction. Needed for mutations, which check
   * their writes (connect targets, cardinality, authorization) before
   * committing. Optional: the WASM binding has none, so it serves reads.
   * A write transaction waits for the writer lock: the wait must honour
   * `timeoutMs` and `signal` (`loraDriver` bounds it).
   */
  begin?(options: RunOptions): Promise<DriverTransaction>;
  /** Plan a statement without running it. Optional: the WASM binding has no `explain()`. */
  explain?(statement: Statement): Promise<QueryPlan>;
  /** The engine's committed change feed (lora-node `db.changes()`). */
  changes?(options: {
    fromLsn?: number;
    signal?: AbortSignal;
  }): AsyncIterable<DriverChangeBatch> & { ready: Promise<void> };
}

/** One change of a committed write, as lora-node reports it. */
export interface DriverChange {
  kind:
    | "nodeCreated"
    | "nodeUpdated"
    | "nodeDeleted"
    | "relationshipCreated"
    | "relationshipUpdated"
    | "relationshipDeleted"
    | "reset";
  id?: number;
  labels?: string[];
  type?: string;
  startId?: number;
  endId?: number;
  properties?: Record<string, unknown>;
  /** For an update: the properties the transaction set and removed. */
  setKeys?: string[];
  removedKeys?: string[];
  /** For a node update: the labels it added and removed. */
  addedLabels?: string[];
  removedLabels?: string[];
}

export interface DriverChangeBatch {
  lsn: number;
  changes: DriverChange[];
}

/** The subset of a LoraDB `Database` the adapter calls. */
export interface LoraDatabaseLike {
  transaction(
    statements: Array<{ query: string; params?: never }>,
    mode?: "read_write" | "read_only",
    options?: { timeoutMs?: number; signal?: AbortSignal },
  ): Promise<QueryResult[]>;
  execute?(
    query: string,
    params?: never,
    options?: { timeoutMs?: number; signal?: AbortSignal },
  ): Promise<QueryResult>;
  stream?(
    query: string,
    params?: never,
    options?: { timeoutMs?: number; signal?: AbortSignal },
  ): {
    columns(): string[] | Promise<string[]>;
    toArray(): Promise<Array<Record<string, unknown>>>;
  };
  explain?(query: string, params?: never): Promise<QueryPlan>;
  changes?(options?: {
    fromLsn?: number;
    signal?: AbortSignal;
  }): AsyncIterable<DriverChangeBatch> & { ready: Promise<void> };
  begin?(
    mode?: "read_write" | "read_only",
    options?: { timeoutMs?: number; signal?: AbortSignal },
  ): Promise<{
    execute(
      query: string,
      params?: never,
      options?: { timeoutMs?: number; signal?: AbortSignal },
    ): Promise<QueryResult>;
    executeMany?(
      statements: Array<{ query: string; params?: never }>,
      options?: { timeoutMs?: number; signal?: AbortSignal },
    ): Promise<QueryResult[]>;
    commit(): Promise<void>;
    rollback(): Promise<void>;
    readonly isOpen: boolean;
  }>;
}

/**
 * Wrap a `Database` from `@loradb/lora-node` or `@loradb/lora-wasm`.
 *
 * ```ts
 * const db = await createDatabase();
 * const lora = new LoraGraphQL({ typeDefs, driver: loraDriver(db) });
 * ```
 */
export function loraDriver(db: LoraDatabaseLike): LoraDriver {
  const execute =
    typeof db.execute === "function" ? db.execute.bind(db) : undefined;
  const stream =
    typeof db.stream === "function" ? db.stream.bind(db) : undefined;
  const driver: LoraDriver = {
    async run(statements, options) {
      const single =
        options.mode === "read" && options.verified && statements.length === 1
          ? statements[0]!
          : undefined;
      // One keyed row of stored properties: a stream answers it on the JS
      // thread faster (~7 µs) than a hop to a libuv worker. Any other read
      // must not run there: db.stream() runs the whole query on the JS
      // thread, so one slow read would block every other request.
      if (single && options.bounded && stream) {
        const rows = stream(
          single.text,
          single.params as never,
          runLimits(options),
        );
        return [{ columns: await rows.columns(), rows: await rows.toArray() }];
      }
      // A single verified read needs no read-only transaction: execute()
      // runs it on a libuv worker at less than half a transaction's cost,
      // and stops at LIMIT as early.
      if (single && execute) {
        return [
          await execute(
            single.text,
            single.params as never,
            runLimits(options),
          ),
        ];
      }
      return db.transaction(
        statements.map((s) => ({ query: s.text, params: s.params as never })),
        options.mode === "read" ? "read_only" : "read_write",
        runLimits(options),
      );
    },
  };
  if (typeof db.begin === "function") {
    const begin = db.begin.bind(db);
    driver.begin = async (options) => {
      const limits = runLimits(options);
      // A current lora-node bounds the wait for the writer lock itself;
      // an older binding ignores the limits, so they are also held here.
      const tx = await bounded(
        begin(options.mode === "read" ? "read_only" : "read_write", limits),
        limits,
      );
      const many =
        typeof tx.executeMany === "function"
          ? tx.executeMany.bind(tx)
          : undefined;
      return {
        execute: (s) => tx.execute(s.text, s.params as never, limits),
        ...(many
          ? {
              executeMany: (statements: Statement[]) =>
                many(
                  statements.map((s) => ({
                    query: s.text,
                    params: s.params as never,
                  })),
                  limits,
                ).catch((err: unknown) => {
                  // The binding names the failing statement's position in
                  // the batch; callers see the error a lone statement
                  // gives. A read-only message (an abort) stays as it is.
                  if (
                    err instanceof Error &&
                    / \(statement \d+ of \d+\)$/.test(err.message)
                  ) {
                    try {
                      err.message = err.message.replace(
                        / \(statement \d+ of \d+\)$/,
                        "",
                      );
                    } catch {
                      // Not ours to rewrite.
                    }
                  }
                  throw err;
                }),
            }
          : {}),
        commit: () => tx.commit(),
        rollback: () => tx.rollback(),
        get isOpen() {
          return tx.isOpen;
        },
      };
    };
  }
  if (typeof db.explain === "function") {
    const explain = db.explain.bind(db);
    driver.explain = (s) => explain(s.text, s.params as never);
  }
  if (typeof db.changes === "function") {
    driver.changes = db.changes.bind(db);
  }
  return driver;
}

/**
 * `driver`, refusing a write transaction when `max` are already waiting
 * for the writer lock: one writer runs at a time, so past saturation a
 * queue only adds latency. `onFull` builds the error to throw. Reads are
 * not counted: they do not wait for the lock.
 */
export function boundedWrites(
  driver: LoraDriver,
  max: number,
  onFull: (waiting: number) => Error,
): LoraDriver {
  // `driver` is absent in an offline instance (schema tooling).
  const begin = (driver as LoraDriver | undefined)?.begin?.bind(driver);
  if (!begin || !Number.isFinite(max)) return driver;
  let waiting = 0;
  // Each method is called on `driver` itself: a driver may be a class
  // instance, whose methods a spread would leave behind.
  const queued = async <T>(work: () => Promise<T>, limit = max): Promise<T> => {
    if (waiting >= limit) throw onFull(waiting);
    waiting++;
    try {
      return await work();
    } finally {
      waiting--;
    }
  };
  // Write transactions begun through this driver and still open: one of
  // them holds the lock.
  const open = new Set<DriverTransaction>();
  const lockHeldByTransaction = () => {
    for (const tx of open) if (!tx.isOpen) open.delete(tx);
    return open.size > 0;
  };
  return {
    // A mutation sent as one write batch (`queued`) waits for the writer
    // lock like a transaction does. It is counted until it returns, not
    // until it holds the lock: unless a transaction holds the lock, one
    // of those counted is the one running, and one more is let in.
    // Other write batches (a worker's bookkeeping, DDL) are never refused.
    run: (statements, options) =>
      options.mode === "write" && options.queued
        ? queued(
            () => driver.run(statements, options),
            lockHeldByTransaction() ? max : max + 1,
          )
        : driver.run(statements, options),
    ...(driver.explain
      ? { explain: (statement: Statement) => driver.explain!(statement) }
      : {}),
    ...(driver.changes
      ? {
          changes: (options: { fromLsn?: number; signal?: AbortSignal }) =>
            driver.changes!(options),
        }
      : {}),
    begin: async (options) => {
      if (options.mode !== "write") return begin(options);
      const tx = await queued(() => begin(options));
      open.add(tx);
      return tx;
    },
  };
}

/**
 * `begin()` bounded by `timeoutMs` and `signal`: a transaction that opens
 * after the caller gave up is rolled back at once, so it never holds the
 * writer lock.
 */
function bounded<T extends { rollback(): Promise<void> }>(
  start: Promise<T>,
  limits: { timeoutMs?: number; signal?: AbortSignal },
): Promise<T> {
  const { timeoutMs, signal } = limits;
  if (!(timeoutMs !== undefined && timeoutMs > 0) && !signal) return start;
  return new Promise<T>((resolve, reject) => {
    let settled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const abandon = (err: unknown) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      signal?.removeEventListener("abort", onAbort);
      void start.then(
        (tx) => tx.rollback().catch(() => undefined),
        () => undefined,
      );
      reject(err);
    };
    const onAbort = () => abandon(signal!.reason);
    if (signal?.aborted) return onAbort();
    signal?.addEventListener("abort", onAbort, { once: true });
    if (timeoutMs !== undefined && timeoutMs > 0) {
      timer = setTimeout(
        () =>
          abandon(
            Object.assign(
              new Error(
                `timed out after ${timeoutMs} ms waiting for the writer lock`,
              ),
              { code: "LORA_TIMEOUT" },
            ),
          ),
        timeoutMs,
      );
    }
    start.then(
      (tx) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        signal?.removeEventListener("abort", onAbort);
        resolve(tx);
      },
      (err: unknown) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        signal?.removeEventListener("abort", onAbort);
        reject(err);
      },
    );
  });
}

function runLimits(options: RunOptions): {
  timeoutMs?: number;
  signal?: AbortSignal;
} {
  const limits: { timeoutMs?: number; signal?: AbortSignal } = {};
  if (options.timeoutMs !== undefined) limits.timeoutMs = options.timeoutMs;
  if (options.signal !== undefined) limits.signal = options.signal;
  return limits;
}
