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
   * checked read-only at startup. The driver may stream it outside a
   * read-only transaction.
   */
  verified?: boolean;
}

/** An open interactive transaction: statements see earlier writes. */
export interface DriverTransaction {
  execute(statement: Statement): Promise<QueryResult>;
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
   */
  begin?(options: RunOptions): Promise<DriverTransaction>;
  /** Plan a statement without running it. Optional: the WASM binding has no `explain()`. */
  explain?(statement: Statement): Promise<QueryPlan>;
}

/** The subset of a LoraDB `Database` the adapter calls. */
export interface LoraDatabaseLike {
  transaction(
    statements: Array<{ query: string; params?: never }>,
    mode?: "read_write" | "read_only",
    options?: { timeoutMs?: number; signal?: AbortSignal },
  ): Promise<QueryResult[]>;
  explain?(query: string, params?: never): Promise<QueryPlan>;
  stream?(
    query: string,
    params?: never,
    options?: { timeoutMs?: number; signal?: AbortSignal },
  ): {
    columns(): string[];
    toArray(): Promise<Array<Record<string, unknown>>>;
  };
  begin?(mode?: "read_write" | "read_only"): Promise<{
    execute(
      query: string,
      params?: never,
      options?: { timeoutMs?: number; signal?: AbortSignal },
    ): Promise<QueryResult>;
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
  const stream =
    typeof db.stream === "function" ? db.stream.bind(db) : undefined;
  const driver: LoraDriver = {
    async run(statements, options) {
      // A single generated read streams: LoraDB 0.15 runs reads under a
      // deadline or inside a transaction without early termination, so a
      // `LIMIT 20` over an index-ordered scan would read the whole label.
      // The stream checks the deadline between rows.
      if (
        stream &&
        options.mode === "read" &&
        options.verified &&
        statements.length === 1
      ) {
        const s = statements[0]!;
        const rows = stream(s.text, s.params as never, runLimits(options));
        return [{ columns: rows.columns(), rows: await rows.toArray() }];
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
      const tx = await begin(
        options.mode === "read" ? "read_only" : "read_write",
      );
      const limits = runLimits(options);
      return {
        execute: (s) => tx.execute(s.text, s.params as never, limits),
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
  return driver;
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
