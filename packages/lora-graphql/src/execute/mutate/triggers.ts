// `@trigger` statements: run for the nodes a mutation wrote, inside its
// transaction, so what they write commits or rolls back with it. A
// statement is the schema author's own Cypher: it is not checked against
// @authorization or the validation rules, as a @cypher mutation is not.

import { viewerKey } from "../../compile/auth.js";
import { bind, type CompileContext } from "../../compile/context.js";
import { name, printExpr } from "../../compile/cypher.js";
import { keyOf } from "../../compile/read/by-keys.js";
import type { QueryResult, Statement } from "../../driver.js";
import { scanParams } from "../../model/cypher-lexer.js";
import type {
  GraphModel,
  MutationOperation,
  NodeType,
  RelationshipField,
  Trigger,
  TriggerOperation,
} from "../../model/types.js";
import { uniqueTogetherTouched } from "../../model/unique-together.js";
import { requestError } from "../../errors.js";
import {
  fillChange,
  type EntityRef,
  type RelationshipRef,
  type WriteChange,
} from "../changes.js";
import { LoraTransaction } from "../transaction.js";
import type { MutationEnv } from "./env.js";
import type { Runner } from "./runner.js";
import { outboxStatements, refId, reportedWrites } from "./reported.js";
import { checkUniqueTogetherOf } from "./unique-together.js";

const anyTriggers = new WeakMap<GraphModel, boolean>();

/** Whether the mutation may fire a trigger: a statement or a function. */
export function hasTriggers(env: MutationEnv): boolean {
  if (env.inTrigger) return false;
  if (env.triggers && Object.keys(env.triggers).length > 0) return true;
  let any = anyTriggers.get(env.model);
  if (any === undefined) {
    any = [...env.model.nodes.values()].some((n) => n.triggers.length > 0);
    anyTriggers.set(env.model, any);
  }
  return any;
}

/** Whether the mutation may call a trigger function. */
export function hasTriggerFunctions(env: MutationEnv): boolean {
  return (
    !env.inTrigger && !!env.triggers && Object.keys(env.triggers).length > 0
  );
}

/** The statement as written, with `$jwt`, `$viewer` and `$operation` bound. */
function triggerBody(
  ctx: CompileContext,
  trigger: Trigger,
  operation: TriggerOperation,
): string {
  const texts = new Map<string, string>();
  for (const p of trigger.params) {
    if (p === "viewer") {
      const expr = viewerKey(ctx);
      texts.set(
        p,
        expr.kind === "param" ? printExpr(expr) : `(${printExpr(expr)})`,
      );
    } else {
      texts.set(
        p,
        printExpr(bind(ctx, p === "jwt" ? (ctx.jwt ?? null) : operation)),
      );
    }
  }
  let body = "";
  let last = 0;
  for (const ref of scanParams(trigger.statement)) {
    body +=
      trigger.statement.slice(last, ref.start) +
      (texts.get(ref.name) ?? `$${ref.name}`);
    last = ref.end;
  }
  return body + trigger.statement.slice(last);
}

/** The trigger statement for `keys`, with `this` bound to each node. */
export function triggerStatement(
  ctx: CompileContext,
  node: NodeType,
  trigger: Trigger,
  operation: TriggerOperation,
  keys: unknown[],
): Statement {
  const body = triggerBody(ctx, trigger, operation);
  // The statement runs as written after the match, not in a CALL { }:
  // the engine refuses writes in a subquery.
  return {
    text:
      `UNWIND ${printExpr(bind(ctx, keys))} AS lora_trigger_key\n` +
      `MATCH (this:${name(node.labels[0]!)}) WHERE this.${name(node.key.property)} = lora_trigger_key\n` +
      body,
    params: ctx.params,
  };
}

/**
 * The text ahead of a CONNECT or DISCONNECT trigger's statement: one row
 * per pair of `pairs` (a list of `[this key, target key]`), with `this`
 * and `target` bound.
 */
export function relationshipTriggerPrefix(
  node: NodeType,
  target: NodeType,
  pairs: string,
): string {
  return (
    `UNWIND ${pairs} AS lora_trigger_pair\n` +
    `MATCH (this:${name(node.labels[0]!)}) WHERE this.${name(node.key.property)} = lora_trigger_pair[0]\n` +
    `MATCH (target:${name(target.labels[0]!)}) WHERE target.${name(target.key.property)} = lora_trigger_pair[1]\n`
  );
}

/**
 * The relationships of `refs` a trigger on `node.field` fires for, as
 * `[this key, target key]` pairs per target type. A relationship counts
 * whichever side's field wrote it: what is compared is its type and the
 * nodes at its start and end.
 */
function pairsFor(
  runner: Runner,
  node: NodeType,
  field: RelationshipField,
  refs: readonly RelationshipRef[],
): Map<NodeType, unknown[][]> {
  const { model } = runner.env;
  const gone = new Set(runner.change.deleted.map(refId));
  const out = new Map<NodeType, Map<string, unknown[]>>();
  for (const ref of refs) {
    if (ref.type !== field.type) continue;
    const [owner, written] = ref.field.split(".") as [string, string];
    const via = model.nodes.get(owner)?.fields.get(written);
    if (via?.kind !== "relationship") continue;
    // The relationship as stored: from its start node to its end node.
    const [start, end] =
      via.direction === "OUT" ? [ref.from, ref.to] : [ref.to, ref.from];
    const [self, other] =
      field.direction === "OUT" ? [start, end] : [end, start];
    if (self.type !== node.name || !field.members.includes(other.type))
      continue;
    // A node deleted since is not there to be `this` or `target`.
    if (gone.has(refId(self)) || gone.has(refId(other))) continue;
    const target = model.nodes.get(other.type);
    if (!target) continue;
    const pairs = out.get(target) ?? new Map<string, unknown[]>();
    pairs.set(`${keyOf(self.key)}\0${keyOf(other.key)}`, [self.key, other.key]);
    out.set(target, pairs);
  }
  return new Map(
    [...out].map(([target, pairs]) => [target, [...pairs.values()]]),
  );
}

/**
 * Run the CREATE and UPDATE triggers for what the mutation wrote so far.
 * A node the mutation created and then updated fires CREATE only.
 */
export async function runWriteTriggers(runner: Runner): Promise<void> {
  if (!hasTriggers(runner.env)) return;
  // What a trigger wrote (a DELETE trigger ran before this) fires nothing.
  const own = (e: EntityRef) => !runner.triggered.has(refId(e));
  const created = group(runner, runner.change.created.filter(own));
  const fresh = new Set(runner.change.created.map(refId));
  const updated = group(
    runner,
    runner.change.updated.filter((e) => own(e) && !fresh.has(refId(e))),
  );
  const fired: Fired[] = [];
  const perNode = (
    node: NodeType,
    trigger: Trigger,
    operation: MutationOperation,
    keys: unknown[],
  ): Fired => [
    trigger,
    (ctx) => triggerStatement(ctx, node, trigger, operation, keys),
  ];
  for (const node of runner.env.model.nodes.values()) {
    for (const trigger of node.triggers) {
      const field =
        trigger.field === undefined
          ? undefined
          : node.fields.get(trigger.field);
      if (field?.kind === "relationship") {
        // Per relationship: the ones that were not there before, and the
        // ones removed with both ends still there.
        for (const [operation, refs] of [
          ["CONNECT", runner.linked],
          ["DISCONNECT", runner.disconnected],
        ] as const) {
          if (!trigger.operations.has(operation)) continue;
          for (const [target, pairs] of pairsFor(runner, node, field, refs)) {
            fired.push([
              trigger,
              (ctx) => ({
                text:
                  relationshipTriggerPrefix(
                    node,
                    target,
                    printExpr(bind(ctx, pairs)),
                  ) + triggerBody(ctx, trigger, operation),
                params: ctx.params,
              }),
            ]);
          }
        }
        continue;
      }
      const creates = trigger.operations.has("CREATE")
        ? (created.get(node) ?? [])
        : [];
      if (creates.length > 0) {
        fired.push(perNode(node, trigger, "CREATE", creates));
      }
      const updates = trigger.operations.has("UPDATE")
        ? (updated.get(node) ?? []).filter((key) =>
            setOneOf(runner, node, key, trigger.fields),
          )
        : [];
      if (updates.length > 0) {
        fired.push(perNode(node, trigger, "UPDATE", updates));
      }
    }
  }
  await fire(runner, fired);
  // Functions after statements, type by type.
  for (const node of runner.env.model.nodes.values()) {
    await call(runner, node, "CREATE", created.get(node) ?? []);
    await call(runner, node, "UPDATE", updated.get(node) ?? []);
  }
}

/** Run the DELETE triggers of `doomed`, while the nodes still exist. */
export async function runDeleteTriggers(
  runner: Runner,
  doomed: ReadonlyArray<{ node: NodeType; keys: unknown[] }>,
): Promise<void> {
  if (!hasTriggers(runner.env)) return;
  const fired: Fired[] = [];
  for (const { node, keys } of doomed) {
    for (const trigger of node.triggers) {
      if (trigger.operations.has("DELETE") && keys.length > 0) {
        fired.push([
          trigger,
          (ctx) => triggerStatement(ctx, node, trigger, "DELETE", keys),
        ]);
      }
    }
  }
  await fire(runner, fired);
  for (const { node, keys } of doomed) await call(runner, node, "DELETE", keys);
}

/**
 * Call the type's trigger function for `operation`, bounded by
 * `triggerTimeoutMs`: it runs while the mutation holds the writer lock.
 * What it writes through the transaction it is given joins the write-set.
 */
async function call(
  runner: Runner,
  node: NodeType,
  operation: MutationOperation,
  keys: unknown[],
): Promise<void> {
  const fn =
    runner.env.triggers?.[node.name]?.[
      operation.toLowerCase() as "create" | "update" | "delete"
    ];
  if (!fn) return;
  if (operation !== "DELETE") {
    // An earlier trigger function may have deleted some of them.
    const gone = new Set(runner.change.deleted.map(refId));
    keys = keys.filter((key) => !gone.has(refId({ type: node.name, key })));
  }
  if (keys.length === 0) return;
  runner.triggerFunctionRan = true;
  // Everything issued so far is in the database before user code reads it.
  await runner.settle();
  const transaction = new LoraTransaction(runner.tx, () => undefined, true);
  const limit = runner.env.triggerTimeoutMs ?? 1000;
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    const running = (async () =>
      fn({
        operation,
        type: node.name,
        keys,
        change: changeSoFar(runner),
        transaction,
        context: runner.env.requestContext,
      }))();
    running.catch(() => undefined);
    await (limit > 0 && Number.isFinite(limit)
      ? Promise.race([
          running,
          new Promise<never>((_, reject) => {
            timer = setTimeout(
              () =>
                reject(
                  requestError(
                    "TIMEOUT",
                    `the ${node.name} ${operation} trigger ran past triggerTimeoutMs (${limit} ms); the mutation is rolled back`,
                    undefined,
                    { triggerTimeoutMs: limit },
                  ),
                ),
              limit,
            );
          }),
        ])
      : running);
  } finally {
    clearTimeout(timer);
  }
  for (const nested of transaction.drain()) merge(runner, nested);
}

/**
 * The write-set as a trigger function sees it: the nodes written so far
 * and the relationships connected and disconnected, which the mutation
 * adds to its change only at its end.
 */
function changeSoFar(runner: Runner): WriteChange {
  const change: WriteChange = {
    ...runner.change,
    created: [...runner.change.created],
    updated: [...runner.change.updated],
    deleted: [...runner.change.deleted],
    connected: [...runner.change.connected],
    disconnected: [...runner.change.disconnected],
  };
  fillChange(change, runner);
  return change;
}

/**
 * Add the write-set of a mutation a trigger ran to the parent's. A node
 * it deleted is no longer created or updated; one the parent created and
 * the trigger deleted never existed for anyone outside the transaction.
 */
function merge(runner: Runner, nested: WriteChange): void {
  const into = runner.change;
  const drop = (list: EntityRef[], id: string): boolean => {
    const at = list.findIndex((e) => refId(e) === id);
    if (at >= 0) list.splice(at, 1);
    return at >= 0;
  };
  for (const ref of nested.deleted) {
    const id = refId(ref);
    const wasCreated = drop(into.created, id);
    drop(into.updated, id);
    if (!wasCreated && !into.deleted.some((e) => refId(e) === id)) {
      into.deleted.push(ref);
    }
    runner.triggered.add(id);
  }
  const seen = new Set(
    [...into.created, ...into.updated, ...into.deleted].map(refId),
  );
  for (const list of ["created", "updated"] as const) {
    for (const ref of nested[list]) {
      const id = refId(ref);
      // Deleted earlier in the transaction and created again: it exists.
      if (list === "created" && drop(into.deleted, id)) seen.delete(id);
      if (seen.has(id)) continue;
      seen.add(id);
      into[list].push(ref);
      runner.triggered.add(id);
    }
  }
  into.connected.push(...nested.connected);
  into.disconnected.push(...nested.disconnected);
  if (nested.before) (into.before ??= []).push(...nested.before);
  into.broad ||= nested.broad;
}

/** A trigger that fires, and its statement for what it fires for. */
type Fired = [Trigger, (ctx: CompileContext) => Statement];

/** Issue every statement at once, then merge what each reports. */
async function fire(runner: Runner, fired: Fired[]): Promise<void> {
  if (fired.length === 0) return;
  const pending = fired.map(([, statement]) => {
    const result = runner.send(statement(runner.ctx()));
    result.catch(() => undefined);
    return result;
  });
  const touched = new Set<NodeType>();
  const made: EntityRef[] = [];
  for (const [i, [trigger]] of fired.entries()) {
    made.push(...report(runner, trigger, await pending[i]!));
    for (const node of uniqueTogetherTouched(
      runner.env.model,
      trigger.statement,
    )) {
      touched.add(node);
    }
  }
  // What a statement wrote is not known node by node: every node of each
  // @uniqueTogether type it may write is compared.
  await checkUniqueTogetherOf(runner.env, runner.tx, [...touched]);
  for (const statement of outboxStatements(runner.env.model, made, () =>
    runner.ctx(),
  )) {
    await runner.send(statement);
  }
}

/**
 * Add the nodes a statement returned as `created` or `updated` to the
 * write-set. A statement that writes and names neither makes the change
 * broad: listeners cannot tell what it touched. Returns the nodes it
 * reported as created.
 */
function report(
  runner: Runner,
  trigger: Trigger,
  result: QueryResult,
): EntityRef[] {
  const reported = reportedWrites(runner.env.model, trigger, result);
  if (reported.broad) runner.change.broad = true;
  const made: EntityRef[] = [];
  const seen = new Set(
    [...runner.change.created, ...runner.change.updated].map(refId),
  );
  for (const list of ["created", "updated"] as const) {
    for (const ref of reported[list]) {
      const id = refId(ref);
      if (seen.has(id)) continue;
      seen.add(id);
      runner.change[list].push(ref);
      runner.triggered.add(id);
      if (list === "created") made.push(ref);
    }
  }
  return made;
}

function group(
  runner: Runner,
  entities: readonly EntityRef[],
): Map<NodeType, unknown[]> {
  const out = new Map<NodeType, Map<string, unknown>>();
  for (const e of entities) {
    const node = runner.env.model.nodes.get(e.type);
    if (!node) continue;
    const keys = out.get(node) ?? new Map<string, unknown>();
    keys.set(keyOf(e.key), e.key);
    out.set(node, keys);
  }
  return new Map([...out].map(([node, keys]) => [node, [...keys.values()]]));
}

/** Whether the node's update set one of `fields` (any, without `fields`). */
function setOneOf(
  runner: Runner,
  node: NodeType,
  key: unknown,
  fields: readonly string[] | undefined,
): boolean {
  if (!fields) return true;
  const set = runner.updatedFields.get(node)?.get(keyOf(key));
  return !!set && fields.some((f) => set.has(f));
}
