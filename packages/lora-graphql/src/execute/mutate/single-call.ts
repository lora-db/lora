// A mutation in one call. A transaction opened with `begin()` holds the
// database's writer lock until `commit()`, and between its calls the lock
// waits for the JavaScript thread, which under load is busy with other
// requests. A mutation whose statements are all known before the first is
// sent needs none of that: `driver.run()` begins, runs them and commits
// in one native step. What JavaScript decided between statements (a rule
// that fails, a hidden key) is then a statement too: `assert()` raises
// inside the transaction and rolls it back.

import { forbidden } from "../../compile/auth.js";
import type {
  DriverTransaction,
  LoraDriver,
  QueryResult,
  RunOptions,
  Statement,
} from "../../driver.js";
import { requestError } from "../../errors.js";
import type { GraphModel, MutationOperation } from "../../model/types.js";

/**
 * A transaction that is one `driver.run()` call until `open()` makes it a
 * real one. In its single form the first batch is the whole mutation: it
 * is committed when the call returns, so `commit()` has nothing left to
 * do, and a failure has already rolled everything back.
 */
export class LazyTransaction implements DriverTransaction {
  readonly #driver: LoraDriver;
  readonly #options: RunOptions;
  #real: DriverTransaction | undefined;
  #ran = false;
  #succeeded = false;
  #open = true;

  constructor(driver: LoraDriver, options: RunOptions) {
    this.#driver = driver;
    this.#options = options;
  }

  /**
   * Whether the single call ran and committed: an error after that can
   * no longer undo the write.
   */
  get committed(): boolean {
    return this.#real === undefined && this.#ran && this.#succeeded;
  }

  /** Whether the mutation is still one call. */
  get single(): boolean {
    return this.#real === undefined;
  }

  /** The mutation needs more than one round trip: begin a transaction. */
  async open(): Promise<void> {
    if (this.#real) return;
    if (this.#ran) {
      throw new Error(
        "lora-graphql: a single-call mutation cannot be reopened",
      );
    }
    this.#real = await this.#driver.begin!(this.#options);
  }

  execute(statement: Statement): Promise<QueryResult> {
    if (this.#real) return this.#real.execute(statement);
    return this.executeMany([statement]).then((results) => results[0]!);
  }

  async executeMany(statements: Statement[]): Promise<QueryResult[]> {
    if (this.#real) {
      if (this.#real.executeMany) return this.#real.executeMany(statements);
      const results: QueryResult[] = [];
      for (const s of statements) results.push(await this.#real.execute(s));
      return results;
    }
    if (this.#ran) {
      // The first call committed. Reaching this is a bug in what was
      // judged a single-call mutation, and must be loud.
      throw new Error(
        "lora-graphql: a single-call mutation sent a second batch after its commit",
      );
    }
    this.#ran = true;
    try {
      const results = await this.#driver.run(statements, this.#options);
      this.#succeeded = true;
      return results;
    } catch (err) {
      // The binding names the failing statement's position in the batch.
      if (err instanceof Error) {
        err.message = err.message.replace(/ \(statement \d+ of \d+\)$/, "");
      }
      throw err;
    } finally {
      this.#open = false;
    }
  }

  commit(): Promise<void> {
    if (this.#real) return this.#real.commit();
    this.#open = false;
    return Promise.resolve();
  }

  rollback(): Promise<void> {
    if (this.#real) return this.#real.rollback();
    this.#open = false;
    return Promise.resolve();
  }

  get isOpen(): boolean {
    return this.#real ? this.#real.isOpen : this.#open;
  }
}

const asserts = new WeakMap<LoraDriver, Promise<boolean>>();

/**
 * Whether the database has `assert()`: asked once per driver. Without it
 * every mutation keeps its transaction and decides in JavaScript.
 */
export function supportsAssert(driver: LoraDriver): Promise<boolean> {
  let known = asserts.get(driver);
  if (!known) {
    known = driver
      .run([{ text: "RETURN assert(true) AS ok", params: {} }], {
        mode: "read",
        verified: true,
      })
      .then(
        ([result]) => result?.rows[0]?.["ok"] === true,
        () => false,
      );
    asserts.set(driver, known);
  }
  return known;
}

const readValidated = new WeakMap<GraphModel, boolean>();

/**
 * Whether reading any node can be refused by a READ validate rule: a
 * payload read would then need a decision after the write, which a single
 * call cannot make.
 */
export function readCanBeRefused(model: GraphModel): boolean {
  let any = readValidated.get(model);
  if (any === undefined) {
    const refuses = (rules?: {
      validate: readonly { operations: ReadonlySet<string> }[];
    }) => !!rules?.validate.some((r) => r.operations.has("READ"));
    any = [...model.nodes.values()].some(
      (n) =>
        refuses(n.authorization) ||
        [...n.fields.values()].some((f) => refuses(f.authorization)),
    );
    readValidated.set(model, any);
  }
  return any;
}

const PREFIX = "lora-graphql:";

/** The message of an `assert()` that stands for a refused rule. */
export function refusal(
  kind: "forbidden" | "hidden" | "taken",
  type: string,
  detail: string,
): string {
  return `${PREFIX}${kind}:${type}:${detail}`;
}

/**
 * The request error an assertion stood for, or `err` when it is not one
 * of ours.
 */
export function refused(model: GraphModel, err: unknown): unknown {
  const message = err instanceof Error ? err.message : String(err);
  const found =
    /assertion failed: lora-graphql:(forbidden|hidden|taken):([^:\s]+):([^\s(]+)/.exec(
      message,
    );
  if (!found) return err;
  const [, kind, type, detail] = found;
  const node = model.nodes.get(type!);
  if (!node) return err;
  if (kind === "taken") {
    return requestError(
      "CONSTRAINT_VIOLATION",
      `${node.name}.${detail} must be unique; the value is taken`,
      undefined,
      { type: node.name, field: detail },
    );
  }
  return forbidden(
    node,
    kind === "hidden" ? "CREATE" : (detail as MutationOperation),
  );
}
