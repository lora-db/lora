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
  Trigger,
} from "../../model/types.js";
import { uniqueTogetherTouched } from "../../model/unique-together.js";
import { requestError } from "../../errors.js";
import { OUTBOX } from "../../events/outbox.js";
import type { EntityRef, WriteChange } from "../changes.js";
import { LoraTransaction } from "../transaction.js";
import type { MutationEnv } from "./env.js";
import type { Runner } from "./runner.js";
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

/** The trigger statement for `keys`, with `this` bound to each node. */
export function triggerStatement(
  ctx: CompileContext,
  node: NodeType,
  trigger: Trigger,
  operation: MutationOperation,
  keys: unknown[],
): Statement {
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
  body += trigger.statement.slice(last);
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
  const fired: Array<[NodeType, Trigger, MutationOperation, unknown[]]> = [];
  for (const node of runner.env.model.nodes.values()) {
    for (const trigger of node.triggers) {
      const creates = trigger.operations.has("CREATE")
        ? (created.get(node) ?? [])
        : [];
      if (creates.length > 0) fired.push([node, trigger, "CREATE", creates]);
      const updates = trigger.operations.has("UPDATE")
        ? (updated.get(node) ?? []).filter((key) =>
            setOneOf(runner, node, key, trigger.fields),
          )
        : [];
      if (updates.length > 0) fired.push([node, trigger, "UPDATE", updates]);
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
  const fired: Array<[NodeType, Trigger, MutationOperation, unknown[]]> = [];
  for (const { node, keys } of doomed) {
    for (const trigger of node.triggers) {
      if (trigger.operations.has("DELETE") && keys.length > 0) {
        fired.push([node, trigger, "DELETE", keys]);
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
        change: runner.change,
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

const refId = (e: EntityRef): string => `${e.type}\0${keyOf(e.key)}`;

/** Issue every statement at once, then merge what each reports. */
async function fire(
  runner: Runner,
  fired: Array<[NodeType, Trigger, MutationOperation, unknown[]]>,
): Promise<void> {
  if (fired.length === 0) return;
  const pending = fired.map(([node, trigger, operation, keys]) => {
    const result = runner.send(
      triggerStatement(runner.ctx(), node, trigger, operation, keys),
    );
    result.catch(() => undefined);
    return result;
  });
  const touched = new Set<NodeType>();
  const made: EntityRef[] = [];
  for (const [i, [, trigger]] of fired.entries()) {
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
  // A statement's CREATE knows nothing of @outbox: the nodes it reports
  // as created become work here, in the same transaction.
  const work = new Map<NodeType, unknown[]>();
  for (const ref of made) {
    const node = runner.env.model.nodes.get(ref.type);
    if (node?.outbox) work.set(node, [...(work.get(node) ?? []), ref.key]);
  }
  for (const [node, keys] of work) {
    const ctx = runner.ctx();
    await runner.run(
      `UNWIND ${printExpr(bind(ctx, keys))} AS k\n` +
        `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = k AND n.${name(OUTBOX.state)} IS NULL\n` +
        `SET n.${name(OUTBOX.state)} = "PENDING"`,
      ctx,
    );
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
  const made: EntityRef[] = [];
  // The result's own columns say what it reports. One that matched no row
  // has none: then the statement's text does.
  const reports =
    result.rows.length > 0
      ? result.columns.includes("created") || result.columns.includes("updated")
      : trigger.reports;
  if (!reports) {
    if (trigger.writes) runner.change.broad = true;
    return made;
  }
  const seen = new Set(
    [...runner.change.created, ...runner.change.updated].map(
      (e) => `${e.type}\0${keyOf(e.key)}`,
    ),
  );
  for (const [column, into] of [
    ["created", runner.change.created],
    ["updated", runner.change.updated],
  ] as const) {
    for (const row of result.rows) {
      const value = row[column];
      for (const item of Array.isArray(value) ? value : [value]) {
        if (item === null || item === undefined) continue;
        const ref = entityOf(runner.env.model, item);
        // Something written that is not a node of the model (a key, a
        // node of an unknown label): listeners cannot be told what.
        if (!ref) {
          if (trigger.writes) runner.change.broad = true;
          continue;
        }
        const id = `${ref.type}\0${keyOf(ref.key)}`;
        if (seen.has(id)) continue;
        seen.add(id);
        into.push(ref);
        runner.triggered.add(id);
        if (column === "created") made.push(ref);
      }
    }
  }
  return made;
}

/** The @node type and key of a node value the engine returned. */
function entityOf(model: GraphModel, value: unknown): EntityRef | undefined {
  const node = value as
    | { labels?: unknown; properties?: Record<string, unknown> }
    | null
    | undefined;
  if (!node || !Array.isArray(node.labels) || !node.properties) {
    return undefined;
  }
  for (const type of model.nodes.values()) {
    if (!node.labels.includes(type.labels[0])) continue;
    const key = node.properties[type.key.property];
    return key === undefined || key === null
      ? undefined
      : { type: type.name, key };
  }
  return undefined;
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
