// A `@cypher` field on Mutation. The statement runs as written, not inside
// a `CALL { }`: LoraDB 0.15 rejects writes in subqueries as read-only.
// Returned nodes are then projected by @key in the same transaction. Inside
// a shared transaction (a caller-owned `lora.begin()`, or every root field
// of an operation under `mutationTransaction: "operation"`) it runs there
// and leaves the commit to its owner, as generated mutations do.

import type { FieldNode } from "graphql";
import { newContext } from "../compile/context.js";
import { bindStatement } from "../compile/read/cypher-field.js";
import { compileByKeys } from "../compile/read/by-keys.js";
import { subSelections } from "../compile/selection.js";
import { requestError } from "../errors.js";
import { assertReadable } from "../schema/guard.js";
import type { Statement } from "../driver.js";
import type { CypherField } from "../model/types.js";
import { uniqueTogetherTouched } from "../model/unique-together.js";
import { checkUniqueTogetherOf } from "./mutate/unique-together.js";
import { runStatement, type MutationEnv } from "./mutate/env.js";

export async function executeCypherMutation(
  env: MutationEnv,
  field: CypherField,
  args: Record<string, unknown>,
  fieldNodes: readonly FieldNode[],
  /** The field's `viewer` rules, checked in the transaction first. */
  guard?: Statement,
): Promise<unknown> {
  const owned = env.transaction;
  if (!owned && !env.driver.begin) {
    throw requestError(
      "DATABASE_ERROR",
      "mutations need a driver with interactive transactions (@loradb/lora-node)",
    );
  }
  const options = {
    jwt: env.jwt,
    degrees: env.degrees,
    maxListArgument: env.maxListArgument,
  };
  const ctx = newContext(env.selection, env.model, options);
  const statement = {
    text: bindStatement(ctx, field, args),
    params: ctx.params,
  };
  const tx =
    owned ??
    (await env.driver.begin!({
      mode: "write",
      timeoutMs: env.timeoutMs,
      signal: env.signal,
    }));
  try {
    if (guard) {
      const check = await runStatement(env, tx, guard);
      if (check.rows[0]?.["allowed"] !== true) {
        throw requestError(
          "FORBIDDEN",
          `not allowed to run ${field.owner}.${field.name}`,
        );
      }
    }
    const result = await runStatement(env, tx, statement);
    // The write-set is unknown: every node of each @uniqueTogether type
    // the statement may write is checked (a scan; the model warns).
    await checkUniqueTogetherOf(
      env,
      tx,
      uniqueTogetherTouched(env.model, field.statement),
    );
    const values = result.rows.map((row) => row[field.columnName]);
    let value: unknown[] = values;
    if (field.node) {
      const node = env.model.nodes.get(field.node)!;
      const keys = values
        .filter((v) => v !== null && v !== undefined)
        .map((v) => {
          const props = (v as { properties?: Record<string, unknown> })
            .properties;
          const key = props?.[node.key.property];
          if (key === undefined) {
            throw requestError(
              "DATABASE_ERROR",
              `${field.name} must return ${node.name} nodes (column ${field.columnName})`,
            );
          }
          return key;
        });
      const read = compileByKeys(
        newContext(env.selection, env.model, options),
        node,
        keys,
        subSelections(fieldNodes),
      );
      value = assertReadable(
        read.shape([
          await runStatement(env, tx, read.statements[0]!),
        ]) as unknown[],
      );
    }
    if (!owned) await tx.commit();
    return field.type.list ? value : (value[0] ?? null);
  } catch (err) {
    if (tx.isOpen) await tx.rollback();
    throw err;
  }
}
