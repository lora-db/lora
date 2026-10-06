// Generated mutations: plan the writes from the input, then run them in one
// interactive transaction, checking what the engine cannot (connect targets
// exist, single and required relationships hold, authorization) before
// commit. Every write is addressed by @key, so the write-set is exact.
//
// This file is the entry point; the parts live in `mutate/`: the write plan
// (plan.ts), the runner that applies it (runner.ts, delete.ts), and the
// checks that follow the writes (checks.ts, unique-together.ts).

import type { FieldNode, GraphQLObjectType, SelectionSetNode } from "graphql";
import {
  authFilter,
  authValidate,
  checkAuthentication,
  claim,
  forbidden,
} from "../compile/auth.js";
import { bind, newContext, type CompileContext } from "../compile/context.js";
import { name, printExpr } from "../compile/cypher.js";
import { keyOf } from "../compile/read/by-keys.js";
import { collectFields, subSelections } from "../compile/selection.js";
import type { DriverTransaction } from "../driver.js";
import { requestError } from "../errors.js";
import type { NodeType } from "../model/types.js";
import {
  mutationNames,
  settable,
  type MutationKind,
} from "../schema/mutations.js";
import type { EntityRef, WriteChange } from "./changes.js";
import { checkUniqueTogether } from "./mutate/checks.js";
import { deleteNodes } from "./mutate/delete.js";
import { runStatement, type MutationEnv } from "./mutate/env.js";
import { andText } from "./mutate/fragments.js";
import type { Input, WritePlan } from "./mutate/plan.js";
import { Runner } from "./mutate/runner.js";

/** The payload's node selection, merged across aliases. */
function payloadSelections(
  env: MutationEnv,
  payloadType: GraphQLObjectType,
  fieldNodes: readonly FieldNode[],
  field: string,
): SelectionSetNode[] {
  const sets: SelectionSetNode[] = [];
  for (const [, nodes] of collectFields(
    env.selection,
    payloadType,
    subSelections(fieldNodes),
  )) {
    if (nodes[0]!.name.value === field) sets.push(...subSelections(nodes));
  }
  return sets;
}

export interface MutationResult {
  payload: unknown;
  change: WriteChange;
}

/**
 * Throw UNAUTHENTICATED / FORBIDDEN when the validate rules for `op` are
 * decided against the request by its claims alone, so a refused write
 * takes no statement (and no writer lock). Rules that depend on the rows
 * are left to the statements.
 */
function settleClaims(
  ctx: CompileContext,
  node: NodeType,
  op: "CREATE" | "UPDATE" | "DELETE",
): void {
  const probe = { ...ctx, params: {}, vars: new Set(ctx.vars) };
  authValidate(probe, node, "n", op, "BEFORE");
  authValidate(probe, node, "n", op, "AFTER");
}

async function lookupViewerKey(
  env: MutationEnv,
  tx: DriverTransaction,
  ctx: CompileContext,
): Promise<unknown> {
  const mapping = env.model.viewer;
  const viewerNode = mapping && env.model.nodes.get(mapping.type);
  if (
    !mapping ||
    !viewerNode ||
    mapping.field === viewerNode.key.name ||
    !env.jwt ||
    ![...env.model.nodes.values()].some((n) => n.key.keyScope)
  ) {
    return undefined;
  }
  const value = claim(ctx, mapping.claim);
  if (typeof value !== "string" && typeof value !== "number") return null;
  const field = viewerNode.fields.get(mapping.field);
  const property = field?.kind === "scalar" ? field.property : mapping.field;
  const rows = (
    await runStatement(env, tx, {
      text:
        `MATCH (v:${name(viewerNode.labels[0]!)}) WHERE v.${name(property)} = $claim\n` +
        `RETURN v.${name(viewerNode.key.property)} AS key LIMIT 1`,
      params: { claim: value },
    })
  ).rows;
  return rows[0]?.["key"] ?? null;
}

export async function executeMutation(
  env: MutationEnv,
  op: MutationKind,
  node: NodeType,
  args: Record<string, unknown>,
  fieldNodes: readonly FieldNode[],
  fieldName: string,
): Promise<MutationResult> {
  const owned = env.transaction;
  if (!owned && !env.driver.begin) {
    throw requestError(
      "DATABASE_ERROR",
      "mutations need a driver with interactive transactions (@loradb/lora-node)",
    );
  }
  const planCtx = newContext(env.selection, env.model, {
    jwt: env.jwt,
    degrees: env.degrees,
    statistics: env.statistics,
    requestContext: env.requestContext,
    maxFilterDepth: env.maxFilterDepth,
    maxListFilter: env.maxListFilter,
    maxStringFilter: env.maxStringFilter,
  });
  const authOp =
    op === "UPSERT"
      ? "CREATE"
      : op === "UPDATE_MANY"
        ? "UPDATE"
        : op === "DELETE_MANY"
          ? "DELETE"
          : op;
  checkAuthentication(planCtx, node, authOp);
  // Rules the claims alone settle against refuse before any statement,
  // as @authentication does (an upsert may turn out an update: skipped).
  if (op !== "UPSERT") settleClaims(planCtx, node, authOp);

  const tx =
    owned ??
    (await env.driver.begin!({
      mode: "write",
      timeoutMs: env.timeoutMs,
      signal: env.signal,
    }));
  const change = emptyChange(op, fieldName);
  // @key(scope: VIEWER) under a @viewer that maps to a non-key field (an
  // opaque subject): the key space is the caller's node's key, looked up
  // once by the claim before anything is written.
  const viewerKey = await lookupViewerKey(env, tx, planCtx);
  if (viewerKey !== undefined) env = { ...env, viewerKey };
  const runner = new Runner(env, tx, change);
  const schema = env.selection.schema;
  const payloadFor = async (
    payloadTypeName: string,
    field: string,
    keys: unknown[],
  ) => {
    const payloadType = schema.getType(payloadTypeName) as GraphQLObjectType;
    return runner.payload(
      node,
      keys,
      payloadSelections(env, payloadType, fieldNodes, field),
    );
  };
  try {
    let payload: unknown;
    const plan = runner.plan();
    switch (op) {
      case "CREATE": {
        const keys = (args["input"] as Input[]).map((input) =>
          plan.create(node, input),
        );
        await runner.apply(plan);
        payload = {
          [mutationNames.createdField(node)]: await payloadFor(
            mutationNames.createPayload(node),
            mutationNames.createdField(node),
            keys,
          ),
          info: runner.info,
        };
        break;
      }
      case "UPDATE": {
        const key = args[node.key.name];
        const field = mutationNames.updatedField(node);
        const done = await runner.update(
          plan,
          node,
          [{ key, input: (args["update"] as Input | undefined) ?? {} }],
          args["adjust"] as Input | undefined,
        );
        await runner.apply(plan);
        await runner.validateUpdated();
        const [value] =
          done.length > 0
            ? await payloadFor(mutationNames.updatePayload(node), field, [key])
            : [null];
        payload = { [field]: value ?? null, info: runner.info };
        break;
      }
      case "UPDATE_MANY": {
        const input = (args["update"] as Input | undefined) ?? {};
        if (
          [...node.fields.values()].some(
            (f) => f.kind === "relationship" && input[f.name] != null,
          )
        ) {
          throw requestError(
            "BAD_USER_INPUT",
            `update${node.name} changes relationships; ${mutationNames.updateMany(node)} changes properties`,
          );
        }
        const keys = await runner.resolveKeys(
          node,
          args["where"] as Input,
          "UPDATE",
          bulkLimit(args["limit"], env.maxBatch),
        );
        const done = await runner.update(
          plan,
          node,
          keys.map((key) => ({ key, input })),
          args["adjust"] as Input | undefined,
        );
        await runner.apply(plan);
        await runner.validateUpdated();
        payload = {
          [mutationNames.createdField(node)]: await payloadFor(
            mutationNames.updateManyPayload(node),
            mutationNames.createdField(node),
            done,
          ),
          info: runner.info,
        };
        break;
      }
      case "UPSERT":
        payload = await upsert(runner, plan, node, args, payloadFor);
        break;
      case "DELETE": {
        // A node the caller cannot see is not there: nothing to delete.
        const ctx = runner.ctx();
        const visible = await runner.run(
          `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = ${printExpr(bind(ctx, args[node.key.name]))}` +
            andText(authFilter(ctx, node, "n", "READ")) +
            andText(authFilter(ctx, node, "n", "DELETE")) +
            `\nRETURN n.${name(node.key.property)} AS key`,
          ctx,
        );
        await deleteNodes(
          runner,
          node,
          visible.map((r) => r["key"]),
        );
        payload = runner.info;
        break;
      }
      case "DELETE_MANY": {
        const keys = await runner.resolveKeys(
          node,
          args["where"] as Input,
          "DELETE",
          bulkLimit(args["limit"], env.maxBatch),
        );
        await deleteNodes(runner, node, keys);
        payload = runner.info;
        break;
      }
    }
    await checkUniqueTogether(runner);
    // Only now, so that any other error is the one a free key gets.
    if (runner.hiddenKey) throw forbidden(runner.hiddenKey, "CREATE");
    if (runner.takenUnique) {
      const { node: taken, field } = runner.takenUnique;
      throw requestError(
        "CONSTRAINT_VIOLATION",
        `${taken.name}.${field.name} must be unique; the value is taken`,
        undefined,
        { type: taken.name, field: field.name },
      );
    }
    if (!owned) await tx.commit();
    fillChange(change, runner);
    return { payload, change };
  } catch (err) {
    if (tx.isOpen) await tx.rollback();
    throw err;
  }
}

/** Create the inputs whose key is new, update the others. */
async function upsert(
  runner: Runner,
  plan: WritePlan,
  node: NodeType,
  args: Record<string, unknown>,
  payloadFor: (
    type: string,
    field: string,
    keys: unknown[],
  ) => Promise<unknown[]>,
): Promise<unknown> {
  const inputs = args["input"] as Input[];
  // Every input writes a node, whether it turns out a create or an update.
  if (inputs.length > runner.env.maxBatch) {
    throw requestError(
      "LIMIT_EXCEEDED",
      `the upsert writes ${inputs.length} nodes; the limit is ${runner.env.maxBatch} nodes per mutation`,
    );
  }
  const keys = inputs.map((i) => i[node.key.name]);
  if (new Set(keys.map(keyOf)).size < keys.length) {
    throw requestError(
      "CONSTRAINT_VIOLATION",
      `the input names a ${node.name} ${node.key.name} twice`,
      undefined,
      { type: node.name, field: node.key.name },
    );
  }
  // Existing nodes the caller may read and update. One the caller can read
  // but not update looks new, and its create fails on the key constraint;
  // one the caller cannot read (even where the UPDATE rules would let it
  // through) takes the create path and fails like a denied create
  // (Runner.moveHiddenKeys): updating it would reveal it (G-20).
  const ctx = runner.ctx();
  const existing = new Set(
    (
      await runner.run(
        `UNWIND ${printExpr(bind(ctx, keys))} AS k\n` +
          `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = k` +
          andText(authFilter(ctx, node, "n", "READ")) +
          andText(authFilter(ctx, node, "n", "UPDATE")) +
          `\nRETURN n.${name(node.key.property)} AS key`,
        ctx,
      )
    ).map((r) => keyOf(r["key"])),
  );
  const updates: Array<{ key: unknown; input: Input }> = [];
  for (const [i, input] of inputs.entries()) {
    if (existing.has(keyOf(keys[i]))) {
      // Fields and relationships set only on create keep the value they
      // were created with.
      const kept = Object.fromEntries(
        Object.entries(input).filter(([k]) => {
          const f = node.fields.get(k);
          return (
            (f?.kind !== "scalar" && f?.kind !== "relationship") ||
            settable(f, "UPDATE")
          );
        }),
      );
      updates.push({ key: keys[i], input: kept });
    } else plan.create(node, input);
  }
  await runner.update(plan, node, updates);
  await runner.apply(plan);
  await runner.validateUpdated();
  const field = mutationNames.createdField(node);
  return {
    [field]: await payloadFor(mutationNames.upsertPayload(node), field, keys),
    info: runner.info,
  };
}

/** A bulk mutation's `limit`: at most `maxBatch`, which is the default. */
function bulkLimit(value: unknown, max: number): number {
  if (value === null || value === undefined) return max;
  if (!Number.isInteger(value) || (value as number) < 0) {
    throw requestError(
      "BAD_USER_INPUT",
      "`limit` must be a non-negative integer",
    );
  }
  if ((value as number) > max) {
    throw requestError(
      "LIMIT_EXCEEDED",
      `\`limit\` is ${value as number}; a bulk mutation reaches at most ${max} nodes`,
    );
  }
  return value as number;
}

function emptyChange(op: MutationKind, field: string): WriteChange {
  const operation =
    op === "UPDATE_MANY" ? "UPDATE" : op === "DELETE_MANY" ? "DELETE" : op;
  return {
    operation,
    field,
    created: [],
    updated: [],
    deleted: [],
    connected: [],
    disconnected: [],
    entities: [],
    types: [],
    relationshipTypes: [],
    broad: false,
  };
}

function fillChange(change: WriteChange, runner: Runner): void {
  change.connected.push(...runner.connected);
  change.disconnected.push(...runner.disconnected);
  const entities = new Map<string, EntityRef>();
  const addEntity = (e: EntityRef) =>
    entities.set(`${e.type}\0${keyOf(e.key)}`, e);
  for (const e of [...change.created, ...change.updated, ...change.deleted])
    addEntity(e);
  for (const r of [...change.connected, ...change.disconnected]) {
    addEntity(r.from);
    addEntity(r.to);
  }
  change.entities = [...entities.values()];
  change.types = [...new Set(change.entities.map((e) => e.type))].sort();
  change.relationshipTypes = [
    ...new Set(
      [...change.connected, ...change.disconnected].map((r) => r.type),
    ),
  ].sort();
}
