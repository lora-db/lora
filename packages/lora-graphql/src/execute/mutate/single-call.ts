// A mutation in one call. A transaction opened with `begin()` holds the
// database's writer lock until `commit()`, and between its calls the lock
// waits for the JavaScript thread, which under load is busy with other
// requests. A mutation whose statements are all known before the first is
// sent needs none of that: `driver.run()` begins, runs them and commits
// in one native step. What JavaScript decided between statements (a rule
// that fails, a hidden key) is then a statement too: `assert()` raises
// inside the transaction and rolls it back.

import { forbidden } from "../../compile/auth.js";
import { lit, printExpr } from "../../compile/cypher.js";
import { keyOf } from "../../compile/read/by-keys.js";
import type {
  DriverTransaction,
  LoraDriver,
  QueryResult,
  RunOptions,
  Statement,
} from "../../driver.js";
import { requestError } from "../../errors.js";
import type { GraphModel, MutationOperation } from "../../model/types.js";
import type { WritePlan } from "./plan.js";

/**
 * A transaction that is one `driver.run()` call until `open()` makes it a
 * real one. In its single form the first batch is the whole mutation: it
 * is committed when the call returns, so `commit()` has nothing left to
 * do, and a failure has already rolled everything back.
 *
 * With `single` false it is never one call, only late: the transaction
 * begins with its first statement, so what a mutation works out before
 * that (its plan, its `@populatedBy` callbacks) does not hold the lock.
 */
export class LazyTransaction implements DriverTransaction {
  readonly #driver: LoraDriver;
  readonly #options: RunOptions;
  readonly #single: boolean;
  #real: DriverTransaction | undefined;
  #opening: Promise<void> | undefined;
  #ran = false;
  #succeeded = false;
  #open = true;

  constructor(driver: LoraDriver, options: RunOptions, single = true) {
    this.#driver = driver;
    this.#options = options;
    this.#single = single;
  }

  /**
   * Whether the single call ran and committed: an error after that can
   * no longer undo the write.
   */
  get committed(): boolean {
    return (
      this.#single && this.#real === undefined && this.#ran && this.#succeeded
    );
  }

  /** Whether the mutation is still one call. */
  get single(): boolean {
    return this.#single && this.#real === undefined;
  }

  /** The mutation needs more than one round trip: begin a transaction. */
  open(): Promise<void> {
    if (this.#real) return Promise.resolve();
    if (this.#ran) {
      return Promise.reject(
        new Error("lora-graphql: a single-call mutation cannot be reopened"),
      );
    }
    // Once, however many statements ask for it together.
    return (this.#opening ??= this.#driver.begin!(this.#options).then(
      (real) => {
        this.#real = real;
      },
    ));
  }

  execute(statement: Statement): Promise<QueryResult> {
    if (this.#real) return this.#real.execute(statement);
    return this.executeMany([statement]).then((results) => results[0]!);
  }

  async executeMany(statements: Statement[]): Promise<QueryResult[]> {
    if (!this.#single && this.#open) await this.open();
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
      const results = await this.#driver.run(statements, {
        ...this.#options,
        queued: true,
      });
      this.#succeeded = true;
      return results;
    } catch (err) {
      throw withoutPosition(err);
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

/**
 * `err` without the position the binding appends to the error of a
 * failing statement of a batch (`(statement 2 of 3)`): callers see the
 * error a lone statement gives. An error whose message is read-only (the
 * DOMException of an aborted request) is passed on as it is.
 */
export function withoutPosition(err: unknown): unknown {
  if (err instanceof Error && / \(statement \d+ of \d+\)$/.test(err.message)) {
    try {
      err.message = err.message.replace(/ \(statement \d+ of \d+\)$/, "");
    } catch {
      // Not ours to rewrite.
    }
  }
  return err;
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
        (err: unknown) => {
          // Only "there is no such function" is an answer. Anything else
          // (a timeout, a database being opened) is asked again.
          const message = err instanceof Error ? err.message : String(err);
          if (!/unknown function|assert/i.test(message)) asserts.delete(driver);
          return false;
        },
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

/**
 * What the guard of a single-call update that writes relationships stands
 * for: the node is not there (or not the caller's to update), so the call
 * rolled back and the mutation answers null, as a transaction would after
 * finding nothing.
 */
export const NOT_THERE = Symbol("lora-graphql: no node to update");

/**
 * Whether `plan`'s links can go out in a single call: when each is
 * known to create its relationship. A transaction asks the database
 * whether the pair was connected already, and applies other rules to a
 * re-connect; here it must be certain beforehand. It is when one end is
 * a node this plan creates (it has no relationships yet) and no two
 * links name the same relationship (one from each side's field). An
 * update's links ask the database instead (`plan.reconnects`). A
 * property the rules refuse depends on what the link found, in
 * JavaScript.
 */
export function linksInOne(plan: WritePlan): boolean {
  const model = plan.model;
  const pairs = new Set<string>();
  const edges = new Set<string>();
  for (const { rel, from, to, refused } of plan.links) {
    if (refused) return false;
    // An update's links tell in the database whether they are new.
    if (plan.reconnects) continue;
    const owner = model.nodes.get(rel.owner)!;
    const target = model.nodes.get(rel.target)!;
    if (!plan.isFresh(owner.name, from) && !plan.isFresh(target.name, to)) {
      return false;
    }
    const pair = `${rel.owner}.${rel.name}\0${keyOf(from)}\0${keyOf(to)}`;
    if (pairs.has(pair)) continue;
    pairs.add(pair);
    const a = `${owner.name}\0${keyOf(from)}`;
    const b = `${target.name}\0${keyOf(to)}`;
    const edge =
      rel.direction === "OUT"
        ? `${rel.type}\0${a}\0${b}`
        : `${rel.type}\0${b}\0${a}`;
    if (edges.has(edge)) return false;
    edges.add(edge);
  }
  return true;
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

/**
 * The assertions of one single call that stand for an error only this
 * mutation can name (the row a check failed on, a target that is not
 * there): each is numbered, and its number is its message.
 */
export class Assertions {
  readonly #errors: Array<(detail: string) => unknown> = [];

  /**
   * The message, a Cypher expression, of an assertion that stands for
   * `error`: {@link Assertions.refusedBy} gives the error back. `detail`
   * is an expression appended to it, for an error that names what the
   * rows held.
   */
  failsAs(error: (detail: string) => unknown, detail?: string): string {
    this.#errors.push(error);
    const prefix = `${PREFIX}at:${this.#errors.length - 1}:`;
    return detail
      ? `${printExpr(lit(prefix))} + ${detail} + ';'`
      : printExpr(lit(`${prefix};`));
  }

  /** The error an assertion of {@link Assertions.failsAs} stood for, if `err` is one. */
  refusedBy(err: unknown): unknown {
    const message = err instanceof Error ? err.message : String(err);
    const found = /assertion failed: lora-graphql:at:(\d+):([^\s(;]*);/.exec(
      message,
    );
    const error = found ? this.#errors[Number(found[1])] : undefined;
    return error ? error(found![2]!) : undefined;
  }
}
