// Statements a subscription compiles once and runs for every change, with
// one parameter (the changed keys) filled in per run.

import { lookupPath } from "../compile/auth.js";
import { stableKey } from "../compile/cache.js";
import type { Statement } from "../driver.js";

/**
 * A statement compiled once for a subscription, reused while its claims
 * and the `$context` values it read are unchanged. `param` names the
 * parameter each run fills in; `share` keys the reads subscribers share.
 */
export interface SubscriptionCompile<C> {
  claims: string;
  statistics: number;
  variables: string | undefined;
  contextReads: Array<[string, unknown]>;
  compiled: C;
  param: string;
  share: string | undefined;
}

/** A cached compile still holds for these `$context` values. */
export function sameContextReads(
  reads: Array<[string, unknown]>,
  context: unknown,
): boolean {
  return reads.every(
    ([path, value]) =>
      stableKey(lookupPath(context, path)) === stableKey(value),
  );
}

/**
 * A compile made with `slot` as one parameter's value, reusable with that
 * parameter filled in per run; undefined unless exactly one parameter
 * holds the slot.
 */
export function slotCompile<C>(
  compiled: C,
  slot: unknown,
  claims: string | undefined,
  statistics: number,
  variables: string | undefined,
  contextReads: Array<[string, unknown]>,
  statement: Statement = compiled as Statement,
): SubscriptionCompile<C> | undefined {
  const slots = Object.keys(statement.params).filter(
    (name) => statement.params[name] === slot,
  );
  if (slots.length !== 1) return undefined;
  const param = slots[0]!;
  const rest = { ...statement.params };
  delete rest[param];
  const fixed = stableKey({ text: statement.text, params: rest });
  return {
    claims: claims ?? "",
    statistics,
    variables,
    contextReads,
    compiled,
    param,
    share: fixed === undefined ? undefined : `s:${param}\0${fixed}`,
  };
}
