// A transaction the caller owns: GraphQL operations and the application's
// own Cypher commit or roll back together. Change events are held until
// the commit, so listeners never see a write that was rolled back.

import type { DriverTransaction, QueryResult } from "../driver.js";
import type { WriteChange } from "./changes.js";

export class LoraTransaction {
  readonly #tx: DriverTransaction;
  readonly #emit: (change: WriteChange) => void;
  readonly #pending: WriteChange[] = [];
  readonly #inTrigger: boolean;

  /**
   * @internal Created by `LoraGraphQL.begin()`, and for a JavaScript
   * trigger over the transaction of the mutation that fired it
   * (`inTrigger`), which that mutation commits.
   */
  constructor(
    tx: DriverTransaction,
    emit: (change: WriteChange) => void,
    inTrigger = false,
  ) {
    this.#tx = tx;
    this.#emit = emit;
    this.#inTrigger = inTrigger;
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
