// Generated mutations: plan the writes from the input, then run them in one
// interactive transaction, checking what the engine cannot (connect targets
// exist, single relationships stay single, authorization) before commit.
// Every write is addressed by @key, so the write-set is exact.

import type { FieldNode, GraphQLObjectType, SelectionSetNode } from "graphql";
import {
  authFilter,
  authValidate,
  checkAuthentication,
  forbidden,
} from "../compile/auth.js";
import { bind, newContext, type CompileContext } from "../compile/context.js";
import { name, printExpr } from "../compile/cypher.js";
import { compileByKeys, keyOf } from "../compile/read.js";
import {
  collectFields,
  subSelections,
  type SelectionContext,
} from "../compile/selection.js";
import type { DriverTransaction, LoraDriver, Statement } from "../driver.js";
import { requestError } from "../errors.js";
import { toStored } from "../model/points.js";
import type {
  GraphModel,
  MutationOperation,
  NodeType,
  RelationshipField,
  ScalarField,
} from "../model/types.js";
import { assertReadable } from "../schema/guard.js";
import {
  mutationNames,
  requiredOnCreate,
  settable,
} from "../schema/mutations.js";
import type { EntityRef, RelationshipRef, WriteChange } from "./changes.js";

export interface MutationEnv {
  model: GraphModel;
  driver: LoraDriver;
  selection: SelectionContext;
  jwt: Record<string, unknown> | undefined;
  timeoutMs: number;
  signal: AbortSignal | undefined;
  degrees: ReadonlyMap<string, number>;
  /** Most nodes one mutation may create, nested creates included. */
  maxBatch: number;
  /** Observes every statement, for logging and tests. */
  onStatement?: ((statement: Statement) => void) | undefined;
}

export interface MutationInfo {
  nodesCreated: number;
  nodesDeleted: number;
  relationshipsCreated: number;
  relationshipsDeleted: number;
}

type Input = Record<string, unknown>;

interface Link {
  rel: RelationshipField;
  /** @key of the node declaring the field. */
  from: unknown;
  /** @key of the target node. */
  to: unknown;
  props: Record<string, unknown>;
}

/** Writes planned from one mutation's input. */
class WritePlan {
  creates = new Map<NodeType, Array<{ key: unknown; props: Input }>>();
  links: Link[] = [];
  disconnects: Array<{
    rel: RelationshipField;
    from: unknown;
    /** Keys to disconnect; undefined: every current target (single fields). */
    to: unknown[] | undefined;
  }> = [];

  constructor(
    readonly model: GraphModel,
    readonly ctx: CompileContext,
  ) {}

  /** Plan a node creation (and its nested writes); returns its key. */
  create(node: NodeType, input: Input): unknown {
    checkAuthentication(this.ctx, node, "CREATE");
    let key = input[node.key.name];
    if (key === undefined || key === null) {
      if (!node.key.generate) {
        throw requestError(
          "BAD_USER_INPUT",
          `${node.name}.${node.key.name} is required`,
        );
      }
      key = globalThis.crypto.randomUUID();
    }
    const props: Input = {};
    for (const f of node.fields.values()) {
      if (f.kind !== "scalar" || f.key) continue;
      const value = input[f.name];
      if (value !== undefined && value !== null && settable(f, "CREATE")) {
        props[f.property] = toStored(f.type, value);
      } else if (f.defaultValue) {
        props[f.property] = storedDefault(f);
      } else if (requiredOnCreate(f) && settable(f, "CREATE")) {
        throw requestError(
          "BAD_USER_INPUT",
          `${node.name}.${f.name} is required`,
        );
      }
    }
    const list = this.creates.get(node) ?? [];
    list.push({ key, props });
    this.creates.set(node, list);
    for (const f of node.fields.values()) {
      if (f.kind !== "relationship") continue;
      const value = input[f.name] as Input | null | undefined;
      if (!value) continue;
      this.relate(node, key, f, value, false);
    }
    return key;
  }

  /** Plan connect / create / disconnect under one relationship field. */
  relate(
    owner: NodeType,
    ownerKey: unknown,
    rel: RelationshipField,
    value: Input,
    update: boolean,
  ): void {
    const target = this.model.nodes.get(rel.target)!;
    const asList = <T>(x: unknown): T[] =>
      x === null || x === undefined
        ? []
        : Array.isArray(x)
          ? (x as T[])
          : [x as T];
    const connect = asList<Input>(value["connect"]);
    const create = asList<Input>(value["create"]);
    const disconnect = value["disconnect"];
    if (!rel.list && connect.length + create.length > 1) {
      throw requestError(
        "BAD_USER_INPUT",
        `${owner.name}.${rel.name} holds one ${target.name}: give connect or create, not both`,
      );
    }
    if (update) {
      if (rel.list) {
        const keys = asList<unknown>(disconnect);
        if (keys.length > 0)
          this.disconnects.push({ rel, from: ownerKey, to: keys });
      } else if (disconnect === true || connect.length + create.length > 0) {
        if (
          disconnect === true &&
          rel.required &&
          connect.length + create.length === 0
        ) {
          throw requestError(
            "BAD_USER_INPUT",
            `${owner.name}.${rel.name} is required: replace it with connect or create instead`,
          );
        }
        // A single relationship is replaced, never doubled.
        this.disconnects.push({ rel, from: ownerKey, to: undefined });
      }
    }
    for (const c of connect) {
      this.links.push({
        rel,
        from: ownerKey,
        to: c[target.key.name],
        props: edgeProps(this.model, rel, c["edge"] as Input | undefined),
      });
    }
    for (const c of create) {
      const key = this.create(target, c["node"] as Input);
      this.links.push({
        rel,
        from: ownerKey,
        to: key,
        props: edgeProps(this.model, rel, c["edge"] as Input | undefined),
      });
    }
  }
}

function edgeProps(
  model: GraphModel,
  rel: RelationshipField,
  edge: Input | undefined,
): Record<string, unknown> {
  if (!rel.properties) return {};
  const props = model.relationshipProperties.get(rel.properties)!;
  const out: Record<string, unknown> = {};
  for (const f of props.fields.values()) {
    const value = edge?.[f.name];
    if (value !== undefined && value !== null && settable(f, "CREATE")) {
      out[f.property] = toStored(f.type, value);
    } else if (f.defaultValue) {
      out[f.property] = storedDefault(f);
    }
  }
  return out;
}

const TEMPORAL_KIND: Partial<Record<string, string>> = {
  Date: "date",
  Time: "time",
  LocalTime: "localtime",
  DateTime: "datetime",
  LocalDateTime: "localdatetime",
  Duration: "duration",
};

/** A @default value as the engine stores it (temporals are tagged). */
function storedDefault(f: ScalarField): unknown {
  const value = f.defaultValue!.value;
  const kind = TEMPORAL_KIND[f.type];
  if (!kind) return value;
  const tag = (v: unknown) => ({ kind, iso: v });
  return Array.isArray(value) ? value.map(tag) : tag(value);
}

const NOW: Partial<Record<string, string>> = {
  DateTime: "datetime()",
  LocalDateTime: "localdatetime()",
  Date: "date()",
};

function timestampSets(
  node: NodeType,
  variable: string,
  op: "CREATE" | "UPDATE",
): string {
  const sets = [...node.fields.values()]
    .filter(
      (f): f is ScalarField => f.kind === "scalar" && !!f.timestamp?.has(op),
    )
    .map((f) => `${name(variable)}.${name(f.property)} = ${NOW[f.type]}`);
  return sets.length > 0 ? `\nSET ${sets.join(", ")}` : "";
}

/** `-[r:T]->(b:Target)` in the direction the owner's field declares. */
function arrow(
  rel: RelationshipField,
  r: string,
  b: string,
  target: NodeType | undefined,
): string {
  const inner = `[${r ? name(r) : ""}:${name(rel.type)}]`;
  const node = target
    ? `(${name(b)}:${name(target.labels[0]!)})`
    : `(${name(b)})`;
  return rel.direction === "OUT" ? `-${inner}->${node}` : `<-${inner}-${node}`;
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

class Runner {
  info: MutationInfo = {
    nodesCreated: 0,
    nodesDeleted: 0,
    relationshipsCreated: 0,
    relationshipsDeleted: 0,
  };
  connected: RelationshipRef[] = [];
  disconnected: RelationshipRef[] = [];

  constructor(
    readonly env: MutationEnv,
    readonly tx: DriverTransaction,
  ) {}

  ctx(): CompileContext {
    return newContext(this.env.selection, this.env.model, {
      jwt: this.env.jwt,
      degrees: this.env.degrees,
    });
  }

  async run(
    text: string,
    ctx: CompileContext,
  ): Promise<Array<Record<string, unknown>>> {
    const statement = { text, params: ctx.params };
    this.env.onStatement?.(statement);
    const result = await this.tx.execute(statement);
    return result.rows;
  }

  async applyCreates(plan: WritePlan): Promise<void> {
    for (const [node, rows] of plan.creates) {
      // Every property goes into the CREATE itself: existence constraints
      // are checked there, before any SET. Rows are grouped by the
      // properties they set, so absent ones are never written as null.
      const groups = new Map<string, Array<{ key: unknown; props: Input }>>();
      for (const row of rows) {
        const shape = Object.keys(row.props).sort().join("\0");
        const group = groups.get(shape) ?? [];
        group.push(row);
        groups.set(shape, group);
      }
      const labels = node.labels.map(name).join(":");
      for (const [shape, group] of groups) {
        const ctx = this.ctx();
        const p = printExpr(bind(ctx, group));
        const props = [
          `${name(node.key.property)}: row.key`,
          ...(shape ? shape.split("\0") : []).map(
            (prop) => `${name(prop)}: row.props.${name(prop)}`,
          ),
        ];
        await this.run(
          `UNWIND ${p} AS row\nCREATE (n:${labels} { ${props.join(", ")} })` +
            timestampSets(node, "n", "CREATE"),
          ctx,
        );
      }
      this.info.nodesCreated += rows.length;
    }
  }

  async applyDisconnects(plan: WritePlan): Promise<void> {
    for (const d of plan.disconnects) {
      const owner = this.env.model.nodes.get(d.rel.owner)!;
      const target = this.env.model.nodes.get(d.rel.target)!;
      const ctx = this.ctx();
      const from = printExpr(bind(ctx, d.from));
      // Nodes the caller cannot see keep their relationships.
      const visible = authFilter(ctx, target, "b", "READ");
      const seen = visible ? ` AND (${printExpr(visible)})` : "";
      let text: string;
      if (d.to) {
        text =
          `UNWIND ${printExpr(bind(ctx, d.to))} AS k\n` +
          `MATCH (a:${name(owner.labels[0]!)})${arrow(d.rel, "r", "b", target)}\n` +
          `WHERE a.${name(owner.key.property)} = ${from} AND b.${name(target.key.property)} = k${seen}\n` +
          `DELETE r RETURN b.${name(target.key.property)} AS key`;
      } else {
        text =
          `MATCH (a:${name(owner.labels[0]!)})${arrow(d.rel, "r", "b", target)}\n` +
          `WHERE a.${name(owner.key.property)} = ${from}${seen}\n` +
          `DELETE r RETURN b.${name(target.key.property)} AS key`;
      }
      const rows = await this.run(text, ctx);
      this.info.relationshipsDeleted += rows.length;
      for (const row of rows) {
        this.disconnected.push(relRef(d.rel, d.from, row["key"]));
      }
    }
  }

  async applyLinks(plan: WritePlan): Promise<void> {
    // Connecting a pair twice in one input is one relationship: the
    // last mention's properties win.
    const unique = new Map<string, Link>();
    for (const link of plan.links) {
      unique.set(
        `${link.rel.owner}.${link.rel.name}\0${keyOf(link.from)}\0${keyOf(link.to)}`,
        link,
      );
    }
    const groups = new Map<RelationshipField, Link[]>();
    for (const link of unique.values()) {
      const list = groups.get(link.rel) ?? [];
      list.push(link);
      groups.set(link.rel, list);
    }
    for (const [rel, links] of groups) {
      const owner = this.env.model.nodes.get(rel.owner)!;
      const target = this.env.model.nodes.get(rel.target)!;
      const ctx = this.ctx();
      const rows = printExpr(
        bind(
          ctx,
          links.map((l) => ({ from: l.from, to: l.to, props: l.props })),
        ),
      );
      // Connecting an already-connected pair replaces its edge. (MERGE is
      // not used: LoraDB 0.15 ignores the bound end node of a MERGE
      // relationship pattern and reuses any edge of that type.)
      const replaced = await this.run(
        `UNWIND ${rows} AS row\n` +
          `MATCH (a:${name(owner.labels[0]!)})${arrow(rel, "r", "b", target)}\n` +
          `WHERE a.${name(owner.key.property)} = row.from AND b.${name(target.key.property)} = row.to\n` +
          `DELETE r RETURN count(r) AS c`,
        ctx,
      );
      this.info.relationshipsDeleted += Number(replaced[0]?.["c"] ?? 0);
      const visible = authFilter(ctx, target, "b", "READ");
      const text =
        `UNWIND ${rows} AS row\n` +
        `MATCH (a:${name(owner.labels[0]!)}) WHERE a.${name(owner.key.property)} = row.from\n` +
        `MATCH (b:${name(target.labels[0]!)}) WHERE b.${name(target.key.property)} = row.to` +
        (visible ? ` AND (${printExpr(visible)})` : "") +
        `\nCREATE (a)${arrow(rel, "r", "b", undefined)}\n` +
        `SET r += row.props\nRETURN row.to AS key`;
      const linked = await this.run(text, ctx);
      if (linked.length < links.length) {
        const found = new Set(linked.map((r) => keyOf(r["key"])));
        const missing = links
          .map((l) => l.to)
          .filter((k) => !found.has(keyOf(k)));
        throw requestError(
          "NOT_FOUND",
          `${owner.name}.${rel.name}: no ${target.name} with ${target.key.name} ${missing
            .map((k) => JSON.stringify(k))
            .join(", ")}`,
          undefined,
          { type: target.name, keys: missing },
        );
      }
      this.info.relationshipsCreated += links.length;
      for (const l of links) this.connected.push(relRef(rel, l.from, l.to));
    }
  }

  /**
   * A single relationship field allows one related node, whichever side
   * the relationship was created from. Check every node that gained one.
   */
  async checkCardinality(plan: WritePlan): Promise<void> {
    const model = this.env.model;
    const touched = new Map<
      string,
      { field: RelationshipField; keys: unknown[] }
    >();
    const add = (field: RelationshipField, key: unknown) => {
      const id = `${field.owner}.${field.name}`;
      const entry = touched.get(id) ?? { field, keys: [] };
      entry.keys.push(key);
      touched.set(id, entry);
    };
    for (const link of plan.links) {
      if (!link.rel.list) add(link.rel, link.from);
      const target = model.nodes.get(link.rel.target)!;
      for (const f of target.fields.values()) {
        if (
          f.kind === "relationship" &&
          !f.list &&
          f.type === link.rel.type &&
          f.target === link.rel.owner &&
          f.direction !== link.rel.direction
        ) {
          add(f, link.to);
        }
      }
    }
    for (const { field, keys } of touched.values()) {
      const owner = model.nodes.get(field.owner)!;
      const target = model.nodes.get(field.target)!;
      const ctx = this.ctx();
      const text =
        `UNWIND ${printExpr(bind(ctx, [...new Map(keys.map((k) => [keyOf(k), k])).values()]))} AS k\n` +
        `MATCH (a:${name(owner.labels[0]!)}) WHERE a.${name(owner.key.property)} = k\n` +
        `WITH a, size([(a)${arrow(field, "", "x", target)} | 1]) AS c WHERE c > 1\n` +
        `RETURN a.${name(owner.key.property)} AS key, c AS count`;
      const rows = await this.run(text, ctx);
      if (rows.length > 0) {
        const row = rows[0]!;
        throw requestError(
          "CONSTRAINT_VIOLATION",
          `${owner.name}.${field.name} holds one ${target.name}, but ${owner.name} ${JSON.stringify(row["key"])} would have ${String(row["count"])}`,
          undefined,
          { type: owner.name, field: field.name },
        );
      }
    }
  }

  /** AFTER rules for `op` on the nodes with `keys`. */
  async validateAfter(
    node: NodeType,
    keys: unknown[],
    op: MutationOperation,
  ): Promise<void> {
    if (keys.length === 0) return;
    const ctx = this.ctx();
    const rule = authValidate(ctx, node, "n", op, "AFTER");
    if (!rule) return;
    const text =
      `UNWIND ${printExpr(bind(ctx, keys))} AS k\n` +
      `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = k\n` +
      `RETURN n.${name(node.key.property)} AS key, ${printExpr(rule)} AS ok`;
    const rows = await this.run(text, ctx);
    if (rows.some((r) => r["ok"] !== true)) throw forbidden(node, op);
  }

  async validateCreated(plan: WritePlan): Promise<void> {
    for (const [node, rows] of plan.creates) {
      await this.validateAfter(
        node,
        rows.map((r) => r.key),
        "CREATE",
      );
    }
  }

  /** Nodes by key, projected for the payload, inside the transaction. */
  async payload(
    node: NodeType,
    keys: unknown[],
    sets: SelectionSetNode[],
  ): Promise<unknown[]> {
    const ctx = this.ctx();
    const compiled = compileByKeys(ctx, node, keys, sets);
    const statement = compiled.statements[0]!;
    this.env.onStatement?.(statement);
    const result = await this.tx.execute(statement);
    return assertReadable(compiled.shape([result]) as unknown[]);
  }

  /** The node's current relationships in the model, for the write-set. */
  async neighbours(node: NodeType, key: unknown): Promise<RelationshipRef[]> {
    // Relationship fields declared on this type, and those on other types
    // that point here: a relationship may be declared on either side.
    const model = this.env.model;
    const own = [...node.fields.values()].filter(
      (f): f is RelationshipField => f.kind === "relationship",
    );
    const inverse = [...model.nodes.values()].flatMap((n) =>
      [...n.fields.values()].filter(
        (f): f is RelationshipField =>
          f.kind === "relationship" && f.target === node.name,
      ),
    );
    if (own.length + inverse.length === 0) return [];
    const ctx = this.ctx();
    const k = printExpr(bind(ctx, key));
    const entries = [
      ...own.map((f, i) => {
        const target = model.nodes.get(f.target)!;
        return `r${i}: [(n)${arrow(f, "", "m", target)} | m.${name(target.key.property)}]`;
      }),
      ...inverse.map((f, i) => {
        const owner = model.nodes.get(f.owner)!;
        // Seen from this node, the declared direction is reversed.
        const back = {
          ...f,
          direction: f.direction === "OUT" ? "IN" : "OUT",
        } as const;
        return `i${i}: [(n)${arrow(back, "", "m", owner)} | m.${name(owner.key.property)}]`;
      }),
    ];
    const rows = await this.run(
      `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = ${k}\nRETURN { ${entries.join(", ")} } AS related`,
      ctx,
    );
    const related = rows[0]?.["related"] as
      | Record<string, unknown[]>
      | undefined;
    if (!related) return [];
    const refs = [
      ...own.flatMap((f, i) =>
        (related[`r${i}`] ?? []).map((to) => relRef(f, key, to)),
      ),
      ...inverse.flatMap((f, i) =>
        (related[`i${i}`] ?? []).map((from) => relRef(f, from, key)),
      ),
    ];
    // A relationship declared on both sides is reported once.
    const seen = new Set<string>();
    return refs.filter((r) => {
      const ends = [
        `${r.from.type}:${keyOf(r.from.key)}`,
        `${r.to.type}:${keyOf(r.to.key)}`,
      ].sort();
      const id = `${r.type}\0${ends.join("\0")}`;
      if (seen.has(id)) return false;
      seen.add(id);
      return true;
    });
  }
}

function relRef(
  rel: RelationshipField,
  from: unknown,
  to: unknown,
): RelationshipRef {
  return {
    type: rel.type,
    field: `${rel.owner}.${rel.name}`,
    from: { type: rel.owner, key: from },
    to: { type: rel.target, key: to },
  };
}

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

export async function executeMutation(
  env: MutationEnv,
  op: MutationOperation,
  node: NodeType,
  args: Record<string, unknown>,
  fieldNodes: readonly FieldNode[],
  fieldName: string,
): Promise<MutationResult> {
  if (!env.driver.begin) {
    throw requestError(
      "DATABASE_ERROR",
      "mutations need a driver with interactive transactions (@loradb/lora-node)",
    );
  }
  const planCtx = newContext(env.selection, env.model, {
    jwt: env.jwt,
    degrees: env.degrees,
  });
  checkAuthentication(planCtx, node, op);
  const plan = new WritePlan(env.model, planCtx);
  const schema = env.selection.schema;

  const tx = await env.driver.begin({
    mode: "write",
    timeoutMs: env.timeoutMs,
    signal: env.signal,
  });
  const runner = new Runner(env, tx);
  const change = emptyChange(op, fieldName);
  try {
    let payload: unknown;
    if (op === "CREATE") {
      const inputs = args["input"] as Input[];
      const keys = inputs.map((input) => plan.create(node, input));
      assertBatch(plan, env.maxBatch);
      assertUniqueKeys(plan);
      await runner.applyCreates(plan);
      await runner.applyLinks(plan);
      await runner.checkCardinality(plan);
      await runner.validateCreated(plan);
      const payloadType = schema.getType(
        mutationNames.createPayload(node),
      ) as GraphQLObjectType;
      const sets = payloadSelections(
        env,
        payloadType,
        fieldNodes,
        mutationNames.createdField(node),
      );
      payload = {
        [mutationNames.createdField(node)]:
          sets.length > 0
            ? await runner.payload(node, keys, sets)
            : keys.map(() => ({})),
        info: runner.info,
      };
    } else if (op === "UPDATE") {
      payload = await update(env, runner, plan, node, args, fieldNodes, change);
    } else {
      await remove(runner, node, args[node.key.name], change);
      payload = runner.info;
    }
    await tx.commit();
    fillChange(change, plan, runner);
    return { payload, change };
  } catch (err) {
    if (tx.isOpen) await tx.rollback();
    throw err;
  }
}

async function update(
  env: MutationEnv,
  runner: Runner,
  plan: WritePlan,
  node: NodeType,
  args: Record<string, unknown>,
  fieldNodes: readonly FieldNode[],
  change: WriteChange,
): Promise<unknown> {
  const key = args[node.key.name];
  const input = args["update"] as Input;
  const updatedField = mutationNames.updatedField(node);

  // Visible to the updater (filter rules) and allowed before the write?
  const ctx = runner.ctx();
  const k = printExpr(bind(ctx, key));
  const filter = authFilter(ctx, node, "n", "UPDATE");
  const before = authValidate(ctx, node, "n", "UPDATE", "BEFORE");
  const found = await runner.run(
    `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = ${k}` +
      (filter ? ` AND (${printExpr(filter)})` : "") +
      `\nRETURN ${before ? printExpr(before) : "true"} AS ok`,
    ctx,
  );
  if (found.length === 0) return { [updatedField]: null, info: runner.info };
  if (found[0]!["ok"] !== true) throw forbidden(node, "UPDATE");

  const set: Input = {};
  const remove: string[] = [];
  for (const f of node.fields.values()) {
    if (!(f.name in input)) continue;
    const value = input[f.name];
    if (f.kind === "relationship") {
      if (value) plan.relate(node, key, f, value as Input, true);
      continue;
    }
    if (f.kind !== "scalar" || !settable(f, "UPDATE")) continue;
    if (value === null) {
      if (f.required) {
        throw requestError(
          "BAD_USER_INPUT",
          `${node.name}.${f.name} is required and cannot be null`,
        );
      }
      remove.push(f.property);
    } else {
      set[f.property] = toStored(f.type, value);
    }
  }
  const uctx = runner.ctx();
  const uk = printExpr(bind(uctx, key));
  const setParam = printExpr(bind(uctx, set));
  await runner.run(
    `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = ${uk}\n` +
      `SET n += ${setParam}` +
      (remove.length > 0
        ? `\nREMOVE ${remove.map((p) => `n.${name(p)}`).join(", ")}`
        : "") +
      timestampSets(node, "n", "UPDATE"),
    uctx,
  );
  change.updated.push({ type: node.name, key });

  assertBatch(plan, env.maxBatch);
  assertUniqueKeys(plan);
  await runner.applyCreates(plan);
  await runner.applyDisconnects(plan);
  await runner.applyLinks(plan);
  await runner.checkCardinality(plan);
  await runner.validateAfter(node, [key], "UPDATE");
  await runner.validateCreated(plan);

  const payloadType = env.selection.schema.getType(
    mutationNames.updatePayload(node),
  ) as GraphQLObjectType;
  const sets = payloadSelections(env, payloadType, fieldNodes, updatedField);
  const [value] =
    sets.length > 0 ? await runner.payload(node, [key], sets) : [{}];
  return { [updatedField]: value ?? null, info: runner.info };
}

async function remove(
  runner: Runner,
  node: NodeType,
  key: unknown,
  change: WriteChange,
): Promise<void> {
  const ctx = runner.ctx();
  const k = printExpr(bind(ctx, key));
  const filter = authFilter(ctx, node, "n", "DELETE");
  const before = authValidate(ctx, node, "n", "DELETE", "BEFORE");
  const found = await runner.run(
    `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = ${k}` +
      (filter ? ` AND (${printExpr(filter)})` : "") +
      `\nRETURN ${before ? printExpr(before) : "true"} AS ok`,
    ctx,
  );
  if (found.length === 0) return;
  if (found[0]!["ok"] !== true) throw forbidden(node, "DELETE");

  runner.disconnected.push(...(await runner.neighbours(node, key)));
  const dctx = runner.ctx();
  const dk = printExpr(bind(dctx, key));
  const rows = await runner.run(
    `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = ${dk}\n` +
      `WITH n, size([(n)--() | 1]) AS rels\nDETACH DELETE n\nRETURN count(*) AS nodes, sum(rels) AS rels`,
    dctx,
  );
  runner.info.nodesDeleted += Number(rows[0]?.["nodes"] ?? 0);
  runner.info.relationshipsDeleted += Number(rows[0]?.["rels"] ?? 0);
  change.deleted.push({ type: node.name, key });
}

function assertBatch(plan: WritePlan, max: number): void {
  let nodes = 0;
  for (const rows of plan.creates.values()) nodes += rows.length;
  if (nodes > max || plan.links.length > max * 10) {
    throw requestError(
      "LIMIT_EXCEEDED",
      `the mutation creates ${nodes} nodes and ${plan.links.length} relationships; the limit is ${max} nodes (and ${max * 10} relationships) per mutation`,
    );
  }
}

function assertUniqueKeys(plan: WritePlan): void {
  for (const [node, rows] of plan.creates) {
    const seen = new Set<string>();
    for (const r of rows) {
      const k = keyOf(r.key);
      if (seen.has(k)) {
        throw requestError(
          "CONSTRAINT_VIOLATION",
          `the input creates ${node.name} ${JSON.stringify(r.key)} twice`,
          undefined,
          { type: node.name, field: node.key.name },
        );
      }
      seen.add(k);
    }
  }
}

function emptyChange(op: MutationOperation, field: string): WriteChange {
  return {
    operation: op,
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

function fillChange(
  change: WriteChange,
  plan: WritePlan,
  runner: Runner,
): void {
  for (const [node, rows] of plan.creates) {
    for (const r of rows) change.created.push({ type: node.name, key: r.key });
  }
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

/** Turn engine constraint errors into CONSTRAINT_VIOLATION naming the field. */
export function mapWriteError(model: GraphModel, err: unknown): unknown {
  const code = (err as { code?: string }).code;
  const message = err instanceof Error ? err.message : String(err);
  if (
    code !== "LORA_UNIQUE_CONSTRAINT" &&
    code !== "LORA_NOT_NULL_CONSTRAINT"
  ) {
    return err;
  }
  const label = /label `([^`]+)`/.exec(message)?.[1];
  const property = /property `([^`]+)`/.exec(message)?.[1];
  const node = [...model.nodes.values()].find((n) => n.labels[0] === label);
  const field = node
    ? [...node.fields.values()].find(
        (f): f is ScalarField => f.kind === "scalar" && f.property === property,
      )
    : undefined;
  const what =
    node && field ? `${node.name}.${field.name}` : `${label}.${property}`;
  return requestError(
    "CONSTRAINT_VIOLATION",
    code === "LORA_UNIQUE_CONSTRAINT"
      ? `${what} must be unique; the value is taken`
      : `${what} is required`,
    err,
    { type: node?.name ?? label, field: field?.name ?? property },
  );
}
