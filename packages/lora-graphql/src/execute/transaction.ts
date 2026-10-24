// A transaction the caller owns: GraphQL operations and the application's
// own Cypher commit or roll back together. Change events are held until
// the commit, so listeners never see a write that was rolled back.

import type { DriverTransaction, QueryResult } from "../driver.js";
import type { EntityRef, WriteChange } from "./changes.js";

/** What the application's own Cypher in a transaction wrote. */
export interface Touched {
  /** Node types (GraphQL type names) whose nodes were written. */
  types?: readonly string[];
  /** Relationship types written, e.g. `FOLLOWS`. */
  relationshipTypes?: readonly string[];
  /** Nodes written, by type and key. Their types count as written too. */
  entities?: readonly EntityRef[];
  /**
   * The write-set is unknown: everything counts as changed. The default
   * when nothing else is named.
   */
  broad?: boolean;
  /** The name listeners see as the change's `field` (default `transaction`). */
  field?: string;
}

export class LoraTransaction {
  readonly #tx: DriverTransaction;
  readonly #emit: (change: WriteChange) => void;
  readonly #pending: WriteChange[] = [];
  readonly #inTrigger: boolean;
  readonly #isType: ((type: string) => boolean) | undefined;

  /**
   * @internal Created by `LoraGraphQL.begin()`, and for a JavaScript
   * trigger over the transaction of the mutation that fired it
   * (`inTrigger`), which that mutation commits.
   */
  constructor(
    tx: DriverTransaction,
    emit: (change: WriteChange) => void,
    inTrigger = false,
    isType?: (type: string) => boolean,
  ) {
    this.#tx = tx;
    this.#emit = emit;
    this.#inTrigger = inTrigger;
    this.#isType = isType;
  }

  /**
   * @internal Whether a trigger owns this handle: mutations run in it
   * fire no trigger themselves.
   */
  get inTrigger(): boolean {
    return this.#inTrigger;
  }

  /** @internal The write-sets recorded so far, handed over. */
  drain(): WriteChange[] {
    return this.#pending.splice(0);
  }

  /** @internal The driver transaction statements run in. */
  get driverTransaction(): DriverTransaction {
    return this.#tx;
  }

  /** False once committed, rolled back, or failed. */
  get isOpen(): boolean {
    return this.#tx.isOpen;
  }

  /** Run the application's own Cypher in the transaction. */
  execute(
    query: string,
    params: Record<string, unknown> = {},
  ): Promise<QueryResult> {
    return this.#tx.execute({ text: query, params });
  }

  /**
   * State what the application's own Cypher in this transaction wrote:
   * it is reported at the commit like a mutation's write-set, to
   * `onWrite` listeners (a response cache), `changes()` and
   * subscriptions, and dropped on a rollback. Without an argument, or
   * with `broad: true`, everything counts as changed.
   */
  touched(written: Touched = {}): void {
    const entities = [...(written.entities ?? [])];
    const types = [
      ...new Set([...(written.types ?? []), ...entities.map((e) => e.type)]),
    ].sort();
    const relationshipTypes = [
      ...new Set(written.relationshipTypes ?? []),
    ].sort();
    if (this.#isType) {
      for (const type of types) {
        if (!this.#isType(type)) {
          throw new Error(
            `touched(): ${type} is not a node type of the schema`,
          );
        }
      }
    }
    this.#pending.push({
      operation: "CYPHER",
      field: written.field ?? "transaction",
      created: [],
      updated: entities,
      deleted: [],
      connected: [],
      disconnected: [],
      entities,
      types,
      relationshipTypes,
      broad:
        written.broad ?? (types.length === 0 && relationshipTypes.length === 0),
    });
  }

  /** @internal A mutation's write-set, reported when the transaction commits. */
  record(change: WriteChange): void {
    this.#pending.push(change);
  }

  async commit(): Promise<void> {
    this.#notInTrigger("commit");
    await this.#tx.commit();
    for (const change of this.#pending.splice(0)) this.#emit(change);
  }

  async rollback(): Promise<void> {
    this.#notInTrigger("rollback");
    this.#pending.length = 0;
    if (this.#tx.isOpen) await this.#tx.rollback();
  }

  #notInTrigger(action: string): void {
    if (this.#inTrigger) {
      throw new Error(
        `a trigger cannot ${action} its transaction: the mutation that fired it does. Throw to roll it back`,
      );
    }
  }
}
