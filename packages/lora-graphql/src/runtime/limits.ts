// What one operation may spend: the estimated cost charged to it (per
// execution, or per event of a subscription) against `maxCost` or the
// request's `budget`, and the deadline and statement slots its root
// fields share (see `execute/budget.ts`).

import type { GraphQLResolveInfo, OperationDefinitionNode } from "graphql";
import { requestError } from "../errors.js";
import { OperationBudget } from "../execute/budget.js";
import type { LoraGraphQLOptions } from "../options.js";

export interface OperationLimitsOptions {
  operationTimeoutMs: number;
  maxConcurrentStatements: number;
  maxCost: number;
  budget: LoraGraphQLOptions["budget"];
  onCost: LoraGraphQLOptions["onCost"];
}

export class OperationLimits {
  readonly #operationTimeoutMs: number;
  readonly #maxConcurrentStatements: number;
  /** Deadline and statement slots per request context and operation. */
  readonly #operations = new WeakMap<
    object,
    Map<OperationDefinitionNode, OperationBudget>
  >();
  readonly #maxCost: number;
  /**
   * Estimated cost spent per execution: keyed by `execute()`'s root value,
   * else by graphql-js's per-execution variable values, never by the
   * context, which a server may reuse across requests.
   */
  readonly #spent = new WeakMap<object, number>();
  /** Root values `execute()` made, one per execution. */
  readonly #executions = new WeakSet<object>();
  /**
   * Cost spent per subscription event (its root value). A subscription
   * reuses one context and operation for every event, so each event is
   * charged on its own, like one operation.
   */
  readonly #eventSpent = new WeakMap<object, number>();
  readonly #budget: LoraGraphQLOptions["budget"];
  readonly #onCost: LoraGraphQLOptions["onCost"];

  constructor(options: OperationLimitsOptions) {
    this.#operationTimeoutMs = options.operationTimeoutMs;
    this.#maxConcurrentStatements = options.maxConcurrentStatements;
    this.#maxCost = options.maxCost;
    this.#budget = options.budget;
    this.#onCost = options.onCost;
  }

  /** Mark `execution` as a root value `execute()` made. */
  track(execution: object): void {
    this.#executions.add(execution);
  }

  /** The estimated cost charged to an execution so far. */
  spent(execution: object): number | undefined {
    return this.#spent.get(execution);
  }

  /**
   * Charge `cost` to the operation: the limit holds per operation, so
   * aliasing a root field many times does not multiply it. A
   * subscription is charged per event.
   */
  charge(
    field: string,
    cost: number,
    context: unknown,
    info?: GraphQLResolveInfo,
  ) {
    let total = cost;
    const event: unknown =
      info?.operation.operation === "subscription" ? info.rootValue : undefined;
    if (event !== null && typeof event === "object") {
      total = (this.#eventSpent.get(event) ?? 0) + cost;
      this.#eventSpent.set(event, total);
    } else if (info) {
      const root: unknown = info.rootValue;
      const execution: unknown =
        root !== null && typeof root === "object" && this.#executions.has(root)
          ? root
          : info.variableValues;
      if (execution !== null && typeof execution === "object") {
        total = (this.#spent.get(execution) ?? 0) + cost;
        this.#spent.set(execution, total);
      }
    }
    const limit = this.#budget?.(context) ?? this.#maxCost;
    try {
      this.#onCost?.({ field, cost, total, limit, context });
    } catch {
      // Observers must not fail the request.
    }
    if (total > limit) {
      throw requestError(
        "COST_EXCEEDED",
        `${field} would bring the operation to about ${Math.ceil(total)} rows touched; the limit is ${limit}. Ask for smaller pages or fewer nested lists.`,
        undefined,
        { cost: Math.ceil(total), maxCost: limit },
      );
    }
  }

  /**
   * The deadline and statement slots of the query `info` belongs to,
   * started by its first root field. Subscriptions run indefinitely and
   * are bounded per statement only.
   */
  operationBudget(
    context: unknown,
    info: GraphQLResolveInfo | undefined,
  ): OperationBudget | undefined {
    if (!info || info.operation.operation === "subscription") return undefined;
    if (context === null || typeof context !== "object") return undefined;
    let byOperation = this.#operations.get(context);
    if (!byOperation) {
      byOperation = new Map();
      this.#operations.set(context, byOperation);
    }
    let budget = byOperation.get(info.operation);
    if (!budget) {
      budget = new OperationBudget(
        this.#operationTimeoutMs,
        this.#maxConcurrentStatements,
      );
      byOperation.set(info.operation, budget);
    }
    budget.active++;
    return budget;
  }

  /**
   * A root field is done. When none is left in flight after the current
   * turn (graphql-js starts a query's root fields together, and the next
   * root field of a mutation in a microtask), the operation is over: a
   * context reused for another request starts a fresh budget.
   */
  leaveOperation(
    context: object,
    info: GraphQLResolveInfo,
    budget: OperationBudget,
  ): void {
    budget.active--;
    if (budget.active > 0) return;
    setTimeout(() => {
      if (budget.active > 0) return;
      budget.dispose();
      const byOperation = this.#operations.get(context);
      if (byOperation?.get(info.operation) === budget) {
        byOperation.delete(info.operation);
      }
    }, 0);
  }
}
