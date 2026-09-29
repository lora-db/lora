// Generated mutations: plan the writes from the input, then run them in one
// interactive transaction, checking what the engine cannot (connect targets
// exist, single and required relationships hold, authorization) before
// commit. Every write is addressed by @key, so the write-set is exact.

import type { FieldNode, GraphQLObjectType, SelectionSetNode } from "graphql";
import {
  authFilter,
  authValidate,
  checkAuthentication,
  checkFieldAuthentication,
  fieldValidate,
  forbidden,
} from "../compile/auth.js";
import { bind, newContext, type CompileContext } from "../compile/context.js";
import {
  and,
  bin,
  fn,
  lit,
  name,
  printExpr,
  prop,
  v,
  type Expr,
} from "../compile/cypher.js";
import { compileNodeWhere } from "../compile/filter.js";
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
  NodeType,
  RelationshipField,
  ScalarField,
} from "../model/types.js";
import { assertReadable } from "../schema/guard.js";
import {
  mutationNames,
  requiredOnCreate,
  settable,
  type MutationKind,
} from "../schema/mutations.js";
import { declared, memberFields } from "../model/relations.js";
import type { EntityRef, RelationshipRef, WriteChange } from "./changes.js";

/** Computes a `@populatedBy` field. */
export type PopulatedByCallback = (args: {
  operation: "CREATE" | "UPDATE";
  /** Node type and field being populated. */
  type: string;
  field: string;
  key: unknown;
  /** The node's input in this mutation. */
  input: Record<string, unknown>;
  /** The GraphQL context. */
  context: unknown;
}) => unknown;

export interface MutationEnv {
  model: GraphModel;
  driver: LoraDriver;
  selection: SelectionContext;
  jwt: Record<string, unknown> | undefined;
  /** The GraphQL context, for `$context` in rules and for callbacks. */
  requestContext: unknown;
  timeoutMs: number;
  signal: AbortSignal | undefined;
  degrees: ReadonlyMap<string, number>;
  /** Most nodes one mutation may create or delete, nested ones included. */
  maxBatch: number;
  callbacks: Readonly<Record<string, PopulatedByCallback>>;
  /**
   * A caller-owned transaction: run inside it and leave the commit to the
   * caller. A failed mutation rolls it back, so it is never half-applied.
   */
  transaction?: DriverTransaction | undefined;
  /** Observes every statement, for logging and tests. */
  onStatement?: ((statement: Statement) => void) | undefined;
}

export interface MutationInfo {
  nodesCreated: number;
  nodesUpdated: number;
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

interface CreateRow {
  key: unknown;
  props: Input;
}

interface EdgeUpdate {
  rel: RelationshipField;
  from: unknown;
  /** Target key; undefined for a single relationship's current target. */
  to: unknown;
  set: Input;
  remove: string[];
}

interface NodeUpdate {
  rel: RelationshipField;
  from: unknown;
  to: unknown;
  input: Input;
}

const asList = <T>(x: unknown): T[] =>
  x === null || x === undefined ? [] : Array.isArray(x) ? (x as T[]) : [x as T];

/** Writes planned from one mutation's input. */
class WritePlan {
  creates = new Map<NodeType, CreateRow[]>();
  links: Link[] = [];
  disconnects: Array<{
    rel: RelationshipField;
    from: unknown;
    /** Keys to disconnect; undefined: every current target (single fields). */
    to: unknown[] | undefined;
  }> = [];
  edgeUpdates: EdgeUpdate[] = [];
  nodeUpdates: NodeUpdate[] = [];
  /** `@populatedBy` callbacks for planned creates. */
  pending: Array<() => Promise<void>> = [];

  constructor(
    readonly env: MutationEnv,
    readonly ctx: CompileContext,
  ) {}

  get model(): GraphModel {
    return this.env.model;
  }

  /**
   * Whether this plan creates the `type` node with `key`. Creates run
   * before links, so such a node has only the relationships this plan
   * gives it.
   */
  isFresh(type: string, key: unknown): boolean {
    for (const [node, rows] of this.creates) {
      if (node.name !== type) continue;
      const k = keyOf(key);
      if (rows.some((r) => keyOf(r.key) === k)) return true;
    }
    return false;
  }

  /**
   * Plan a node creation (and its nested writes); returns its key.
   * `parent` is the relationship a nested create hangs from: it satisfies
   * the new node's required field pointing back.
   */
  create(node: NodeType, input: Input, parent?: RelationshipField): unknown {
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
      if (value !== undefined && value !== null) {
        if (!settable(f, "CREATE")) {
          throw requestError(
            "BAD_USER_INPUT",
            `${node.name}.${f.name} cannot be set on create`,
          );
        }
        checkFieldAuthentication(this.ctx, node.name, f, "CREATE");
        props[f.property] = toStored(f, value);
      } else if (f.populatedBy?.operations.has("CREATE")) {
        this.pending.push(async () => {
          props[f.property] = toStored(
            f,
            await this.callback(f, "CREATE", node, key, input),
          );
        });
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
      const satisfied =
        parent !== undefined && inverseFields(this.model, parent).includes(f);
      const given = (op: string) => {
        const part = value?.[op];
        if (part == null) return false;
        // Member-keyed for interfaces and unions.
        return this.model.abstracts.has(f.target)
          ? Object.values(part as Input).some((x) => x != null)
          : true;
      };
      if (
        f.required &&
        !f.list &&
        !satisfied &&
        !given("connect") &&
        !given("create")
      ) {
        throw requestError(
          "BAD_USER_INPUT",
          `${node.name}.${f.name} is required: connect or create one`,
        );
      }
      if (value) this.relate(node, key, f, value, false);
    }
    return key;
  }

  async callback(
    f: ScalarField,
    operation: "CREATE" | "UPDATE",
    node: NodeType,
    key: unknown,
    input: Input,
  ): Promise<unknown> {
    const cb = this.env.callbacks[f.populatedBy!.callback]!;
    return cb({
      operation,
      type: node.name,
      field: f.name,
      key,
      input,
      context: this.env.requestContext,
    });
  }

  /** Plan connect / create / update / disconnect under one relationship field. */
  relate(
    owner: NodeType,
    ownerKey: unknown,
    rel: RelationshipField,
    value: Input,
    update: boolean,
  ): void {
    if (this.model.abstracts.has(rel.target)) {
      // Member-keyed input: each member's part goes to its concrete copy.
      const members = memberFields(this.model, rel);
      const pick = (op: string, member: string) =>
        (value[op] as Input | null | undefined)?.[member];
      const adding = members.reduce(
        (n, m) =>
          n +
          asList(pick("connect", m.target)).length +
          asList(pick("create", m.target)).length,
        0,
      );
      if (!rel.list && adding > 1) {
        throw requestError(
          "BAD_USER_INPUT",
          `${owner.name}.${rel.name} holds one ${rel.target}: connect or create one`,
        );
      }
      // Replacing a single relationship clears every member's edge.
      const replace =
        !rel.list && update && (adding > 0 || value["disconnect"] === true);
      for (const m of members) {
        this.relate(
          owner,
          ownerKey,
          m,
          {
            connect: pick("connect", m.target),
            create: pick("create", m.target),
            disconnect: rel.list
              ? pick("disconnect", m.target)
              : replace
                ? true
                : undefined,
          },
          update,
        );
      }
      return;
    }
    const target = this.model.nodes.get(rel.target)!;
    const connect = asList<Input>(value["connect"]);
    const create = asList<Input>(value["create"]);
    const updates = asList<Input>(value["update"]);
    const disconnect = value["disconnect"];
    if (!rel.list && connect.length + create.length > 1) {
      throw requestError(
        "BAD_USER_INPUT",
        `${owner.name}.${rel.name} holds one ${target.name}: give connect or create, not both`,
      );
    }
    if (connect.length + create.length > 0) {
      checkAuthentication(this.ctx, owner, "CREATE_RELATIONSHIP");
      checkAuthentication(this.ctx, target, "CREATE_RELATIONSHIP");
    }
    if (update) {
      if (rel.list) {
        const keys = asList<unknown>(disconnect);
        if (keys.length > 0) {
          checkAuthentication(this.ctx, owner, "DELETE_RELATIONSHIP");
          this.disconnects.push({ rel, from: ownerKey, to: keys });
        }
      } else if (disconnect === true || connect.length + create.length > 0) {
        if (
          disconnect === true &&
          rel.required &&
          !rel.via &&
          connect.length + create.length === 0
        ) {
          throw requestError(
            "BAD_USER_INPUT",
            `${owner.name}.${rel.name} is required: replace it with connect or create instead`,
          );
        }
        checkAuthentication(this.ctx, owner, "DELETE_RELATIONSHIP");
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
      const key = this.create(target, c["node"] as Input, rel);
      this.links.push({
        rel,
        from: ownerKey,
        to: key,
        props: edgeProps(this.model, rel, c["edge"] as Input | undefined),
      });
    }
    for (const u of updates) {
      const to = rel.list ? u[target.key.name] : undefined;
      const edge = u["edge"] as Input | null | undefined;
      if (edge && rel.properties) {
        const props = this.model.relationshipProperties.get(rel.properties)!;
        const set: Input = {};
        const remove: string[] = [];
        for (const f of props.fields.values()) {
          if (!(f.name in edge) || !settable(f, "UPDATE")) continue;
          if (edge[f.name] === null) {
            if (f.required) {
              throw requestError(
                "BAD_USER_INPUT",
                `${props.name}.${f.name} is required and cannot be null`,
              );
            }
            remove.push(f.property);
          } else {
            set[f.property] = toStored(f, edge[f.name]);
          }
        }
        this.edgeUpdates.push({ rel, from: ownerKey, to, set, remove });
      }
      const node = u["node"] as Input | null | undefined;
      if (node) this.nodeUpdates.push({ rel, from: ownerKey, to, input: node });
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
      out[f.property] = toStored(f, value);
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
  const tag = (x: unknown) => ({ kind, iso: x });
  return Array.isArray(value) ? value.map(tag) : tag(value);
}

const NOW: Partial<Record<string, string>> = {
  DateTime: "datetime()",
  LocalDateTime: "localdatetime()",
  Date: "date()",
};

/** `@timestamp` fields for `op`: [property, now-expression]. */
function timestamps(
  node: NodeType,
  op: "CREATE" | "UPDATE",
): Array<[string, string]> {
  return [...node.fields.values()]
    .filter(
      (f): f is ScalarField => f.kind === "scalar" && !!f.timestamp?.has(op),
    )
    .map((f) => [f.property, NOW[f.type]!]);
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

/**
 * `b:Label`: LoraDB 0.15 ignores the labels of every node after the first
 * in a MATCH pattern, so expansions test them explicitly.
 */
function labelTest(variable: string, node: NodeType): string {
  return `${name(variable)}:${name(node.labels[0]!)}`;
}

/**
 * `MATCH (a:Owner) WHERE a.key = <key>` then `MATCH (a)-[r:T]->(b:Target)
 * WHERE b:Target`, for callers to extend with ` AND ...`. The key test gets
 * its own MATCH so it becomes an index seek: LoraDB 0.15 only seeks when
 * the test sits directly on a node scan, and one pattern with the test in
 * its WHERE expands every relationship of the type first.
 */
function seekThenExpand(
  a: string,
  owner: NodeType,
  key: string,
  rel: RelationshipField,
  r: string,
  b: string,
  target: NodeType,
): string {
  return (
    `MATCH (${name(a)}:${name(owner.labels[0]!)}) WHERE ${name(a)}.${name(owner.key.property)} = ${key}\n` +
    `MATCH (${name(a)})${arrow(rel, r, b, target)}\n` +
    `WHERE ${labelTest(b, target)}`
  );
}

/** `AND (<expr>)`, or nothing. */
const andText = (e: Expr | undefined) => (e ? ` AND (${printExpr(e)})` : "");

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

class Runner {
  info: MutationInfo = {
    nodesCreated: 0,
    nodesUpdated: 0,
    nodesDeleted: 0,
    relationshipsCreated: 0,
    relationshipsDeleted: 0,
  };
  connected: RelationshipRef[] = [];
  disconnected: RelationshipRef[] = [];
  /** Nodes updated per type, for AFTER validation. */
  updated = new Map<NodeType, { keys: unknown[]; fields: Set<string> }>();

  constructor(
    readonly env: MutationEnv,
    readonly tx: DriverTransaction,
    readonly change: WriteChange,
  ) {}

  ctx(): CompileContext {
    return newContext(this.env.selection, this.env.model, {
      jwt: this.env.jwt,
      degrees: this.env.degrees,
      requestContext: this.env.requestContext,
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

  plan(): WritePlan {
    return new WritePlan(this.env, this.ctx());
  }

  /** Every planned write, in order, with the checks that follow them. */
  async apply(plan: WritePlan): Promise<void> {
    for (const task of plan.pending) await task();
    assertBatch(plan, this.env.maxBatch);
    assertUniqueKeys(plan);
    await this.applyCreates(plan);
    await this.applyDisconnects(plan);
    await this.applyLinks(plan);
    await this.applyEdgeUpdates(plan);
    await this.applyNodeUpdates(plan);
    await this.checkCardinality(plan);
    await this.checkRequired();
    await this.validateCreated(plan);
  }

  async applyCreates(plan: WritePlan): Promise<void> {
    for (const [node, rows] of plan.creates) {
      // Every property goes into the CREATE itself (timestamps included):
      // existence constraints are checked there, before any SET. Rows are
      // grouped by the properties they set, so absent ones are never
      // written as null.
      const groups = new Map<string, CreateRow[]>();
      for (const row of rows) {
        const shape = Object.keys(row.props).sort().join("\0");
        const group = groups.get(shape) ?? [];
        group.push(row);
        groups.set(shape, group);
      }
      const labels = node.labels.map(name).join(":");
      const stamped = timestamps(node, "CREATE");
      for (const [shape, group] of groups) {
        const ctx = this.ctx();
        const p = printExpr(bind(ctx, group));
        const props = [
          `${name(node.key.property)}: row.key`,
          ...(shape ? shape.split("\0") : []).map(
            (prop) => `${name(prop)}: row.props.${name(prop)}`,
          ),
          ...stamped.map(([prop, now]) => `${name(prop)}: ${now}`),
        ];
        await this.run(
          `UNWIND ${p} AS row\nCREATE (n:${labels} { ${props.join(", ")} })`,
          ctx,
        );
      }
      this.info.nodesCreated += rows.length;
      for (const r of rows) {
        this.change.created.push({ type: node.name, key: r.key });
      }
    }
  }

  async applyDisconnects(plan: WritePlan): Promise<void> {
    for (const d of plan.disconnects) {
      const owner = this.env.model.nodes.get(d.rel.owner)!;
      const target = this.env.model.nodes.get(d.rel.target)!;
      const ctx = this.ctx();
      const from = printExpr(bind(ctx, d.from));
      // Nodes the caller cannot see keep their relationships, and both
      // ends must allow DELETE_RELATIONSHIP when their rules say so.
      const guard =
        andText(authFilter(ctx, target, "b", "READ")) +
        andText(authFilter(ctx, target, "b", "DELETE_RELATIONSHIP")) +
        andText(authFilter(ctx, owner, "a", "DELETE_RELATIONSHIP"));
      const matchText =
        (d.to ? `UNWIND ${printExpr(bind(ctx, d.to))} AS k\n` : "") +
        seekThenExpand("a", owner, from, d.rel, "r", "b", target) +
        (d.to ? ` AND b.${name(target.key.property)} = k` : "") +
        `${guard}\n`;
      const returned = `b.${name(target.key.property)} AS key`;
      const pairs = (rows: Array<Record<string, unknown>>) =>
        rows.map((r) => ({ from: d.from, to: r["key"] }));
      if (this.endRules(owner, target, "DELETE_RELATIONSHIP", "BEFORE")) {
        const doomed = await this.run(`${matchText}RETURN ${returned}`, ctx);
        await this.checkEnds(
          "DELETE_RELATIONSHIP",
          owner,
          target,
          pairs(doomed),
          "BEFORE",
        );
      }
      const rows = await this.run(
        `${matchText}DELETE r RETURN ${returned}`,
        ctx,
      );
      await this.checkEnds(
        "DELETE_RELATIONSHIP",
        owner,
        target,
        pairs(rows),
        "AFTER",
      );
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
      const pairs = links.map((l) => ({ from: l.from, to: l.to }));
      await this.checkEnds(
        "CREATE_RELATIONSHIP",
        owner,
        target,
        pairs,
        "BEFORE",
      );
      const ctx = this.ctx();
      // Connecting an already-connected pair keeps one relationship and
      // updates its properties. It is replaced rather than merged: LoraDB
      // 0.15 ignores the bound end node of a MERGE relationship pattern.
      // A node this plan creates has no relationship to replace.
      const old = links.filter(
        (l) =>
          !plan.isFresh(rel.owner, l.from) && !plan.isFresh(rel.target, l.to),
      );
      const existing =
        old.length === 0
          ? []
          : await this.run(
              `UNWIND ${printExpr(
                bind(
                  ctx,
                  old.map((l) => ({ from: l.from, to: l.to })),
                ),
              )} AS row\n` +
                seekThenExpand("a", owner, "row.from", rel, "r", "b", target) +
                ` AND b.${name(target.key.property)} = row.to\n` +
                `WITH row, r, properties(r) AS old DELETE r RETURN row.from AS from, row.to AS to, old`,
              ctx,
            );
      const previous = new Map<string, Input>();
      for (const row of existing) {
        previous.set(
          `${keyOf(row["from"])}\0${keyOf(row["to"])}`,
          (row["old"] as Input | null) ?? {},
        );
      }
      const lctx = this.ctx();
      const rows = printExpr(
        bind(
          lctx,
          links.map((l) => ({
            from: l.from,
            to: l.to,
            props: {
              ...(previous.get(`${keyOf(l.from)}\0${keyOf(l.to)}`) ?? {}),
              ...l.props,
            },
          })),
        ),
      );
      const text =
        `UNWIND ${rows} AS row\n` +
        `MATCH (a:${name(owner.labels[0]!)}) WHERE a.${name(owner.key.property)} = row.from` +
        andText(authFilter(lctx, owner, "a", "CREATE_RELATIONSHIP")) +
        `\nMATCH (b:${name(target.labels[0]!)}) WHERE b.${name(target.key.property)} = row.to` +
        andText(authFilter(lctx, target, "b", "READ")) +
        andText(authFilter(lctx, target, "b", "CREATE_RELATIONSHIP")) +
        `\nCREATE (a)${arrow(rel, "r", "b", undefined)}\n` +
        `SET r += row.props\nRETURN row.to AS key`;
      const linked = await this.run(text, lctx);
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
      await this.checkEnds(
        "CREATE_RELATIONSHIP",
        owner,
        target,
        pairs,
        "AFTER",
      );
      this.info.relationshipsCreated += links.length - existing.length;
      for (const l of links) this.connected.push(relRef(rel, l.from, l.to));
    }
  }

  /** Whether either end has validate rules for `op` at `when`. */
  endRules(
    owner: NodeType,
    target: NodeType,
    op: "CREATE_RELATIONSHIP" | "DELETE_RELATIONSHIP",
    when: "BEFORE" | "AFTER",
  ): boolean {
    return [owner, target].some((n) =>
      n.authorization?.validate.some(
        (r) => r.operations.has(op) && r.when.has(when),
      ),
    );
  }

  /**
   * Validate rules for connecting or disconnecting, on both ends of each
   * pair: FORBIDDEN when either end fails them.
   */
  async checkEnds(
    op: "CREATE_RELATIONSHIP" | "DELETE_RELATIONSHIP",
    owner: NodeType,
    target: NodeType,
    pairs: Array<{ from: unknown; to: unknown }>,
    when: "BEFORE" | "AFTER",
  ): Promise<void> {
    if (pairs.length === 0) return;
    const ctx = this.ctx();
    const a = authValidate(ctx, owner, "a", op, when);
    const b = authValidate(ctx, target, "b", op, when);
    if (!a && !b) return;
    const rows = await this.run(
      `UNWIND ${printExpr(bind(ctx, pairs))} AS row\n` +
        `MATCH (a:${name(owner.labels[0]!)}) WHERE a.${name(owner.key.property)} = row.from\n` +
        `MATCH (b:${name(target.labels[0]!)}) WHERE b.${name(target.key.property)} = row.to\n` +
        `RETURN ${a ? printExpr(a) : "true"} AS a, ${b ? printExpr(b) : "true"} AS b`,
      ctx,
    );
    for (const row of rows) {
      if (row["a"] !== true) throw forbidden(owner, op);
      if (row["b"] !== true) throw forbidden(target, op);
    }
  }

  /** `update: [{ key, edge }]`: properties of connected pairs, in place. */
  async applyEdgeUpdates(plan: WritePlan): Promise<void> {
    for (const u of plan.edgeUpdates) {
      const owner = this.env.model.nodes.get(u.rel.owner)!;
      const target = this.env.model.nodes.get(u.rel.target)!;
      const ctx = this.ctx();
      const text =
        seekThenExpand(
          "a",
          owner,
          printExpr(bind(ctx, u.from)),
          u.rel,
          "r",
          "b",
          target,
        ) +
        (u.to !== undefined
          ? ` AND b.${name(target.key.property)} = ${printExpr(bind(ctx, u.to))}`
          : "") +
        andText(authFilter(ctx, target, "b", "READ")) +
        `\nSET r += ${printExpr(bind(ctx, u.set))}` +
        (u.remove.length > 0
          ? `\nREMOVE ${u.remove.map((p) => `r.${name(p)}`).join(", ")}`
          : "") +
        `\nRETURN b.${name(target.key.property)} AS key`;
      const rows = await this.run(text, ctx);
      if (rows.length === 0) throw notConnected(owner, u.rel, target, u.to);
      for (const row of rows) {
        this.connected.push(relRef(u.rel, u.from, row["key"]));
      }
    }
  }

  /** `update: [{ key, node }]`: connected nodes, through the update path. */
  async applyNodeUpdates(plan: WritePlan): Promise<void> {
    for (const u of plan.nodeUpdates) {
      const owner = this.env.model.nodes.get(u.rel.owner)!;
      const target = this.env.model.nodes.get(u.rel.target)!;
      const ctx = this.ctx();
      const rows = await this.run(
        seekThenExpand(
          "a",
          owner,
          printExpr(bind(ctx, u.from)),
          u.rel,
          "",
          "b",
          target,
        ) +
          (u.to !== undefined
            ? ` AND b.${name(target.key.property)} = ${printExpr(bind(ctx, u.to))}`
            : "") +
          `\nRETURN DISTINCT b.${name(target.key.property)} AS key`,
        ctx,
      );
      if (rows.length === 0) throw notConnected(owner, u.rel, target, u.to);
      const sub = this.plan();
      const done = await this.update(
        sub,
        target,
        rows.map((r) => ({ key: r["key"], input: u.input })),
      );
      if (done.length < rows.length) {
        throw notConnected(owner, u.rel, target, u.to);
      }
      await this.apply(sub);
    }
  }

  /**
   * Update nodes by key: visibility (filter rules) and BEFORE rules, then
   * one SET per shape of input. Returns the keys it updated; keys the
   * caller cannot see are skipped. Relationship inputs go into `plan`.
   */
  async update(
    plan: WritePlan,
    node: NodeType,
    rows: Array<{ key: unknown; input: Input }>,
    adjust?: Input,
  ): Promise<unknown[]> {
    if (rows.length === 0) return [];
    const ctx0 = this.ctx();
    checkAuthentication(ctx0, node, "UPDATE");
    const fields = new Set<string>();
    for (const { input } of rows) {
      for (const name of Object.keys(input)) fields.add(name);
    }
    for (const name of Object.keys(adjust ?? {})) fields.add(name);
    const written = [...fields]
      .map((n) => node.fields.get(n))
      .filter((f): f is ScalarField => f?.kind === "scalar" && !f.key);
    for (const f of written) {
      checkFieldAuthentication(ctx0, node.name, f, "UPDATE");
    }

    // Visible to the updater (filter rules) and allowed before the write?
    const ctx = this.ctx();
    const before = and(
      authValidate(ctx, node, "n", "UPDATE", "BEFORE"),
      ...written.map((f) =>
        fieldValidate(ctx, node, f, "n", "UPDATE", "BEFORE"),
      ),
    );
    const found = await this.run(
      `UNWIND ${printExpr(
        bind(
          ctx,
          rows.map((r) => r.key),
        ),
      )} AS k\n` +
        `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = k` +
        andText(authFilter(ctx, node, "n", "UPDATE")) +
        `\nRETURN n.${name(node.key.property)} AS key, ${before ? printExpr(before) : "true"} AS ok`,
      ctx,
    );
    if (found.some((r) => r["ok"] !== true)) throw forbidden(node, "UPDATE");
    const visible = new Set(found.map((r) => keyOf(r["key"])));
    const todo = rows.filter((r) => visible.has(keyOf(r.key)));
    if (todo.length === 0) return [];

    // Per row: properties to set (with @populatedBy) and to remove.
    const prepared: Array<{ key: unknown; set: Input; remove: string[] }> = [];
    for (const { key, input } of todo) {
      const set: Input = {};
      const remove: string[] = [];
      for (const f of node.fields.values()) {
        if (f.kind === "relationship") {
          const value = input[f.name];
          if (value) plan.relate(node, key, f, value as Input, true);
          continue;
        }
        if (f.kind !== "scalar" || f.key) continue;
        if (f.name in input) {
          if (!settable(f, "UPDATE")) {
            throw requestError(
              "BAD_USER_INPUT",
              `${node.name}.${f.name} cannot be changed after create`,
            );
          }
          if (adjust?.[f.name] != null) {
            throw requestError(
              "BAD_USER_INPUT",
              `${node.name}.${f.name} is both set and adjusted`,
            );
          }
          const value = input[f.name];
          if (value === null) {
            if (f.required) {
              throw requestError(
                "BAD_USER_INPUT",
                `${node.name}.${f.name} is required and cannot be null`,
              );
            }
            remove.push(f.property);
          } else {
            set[f.property] = toStored(f, value);
          }
        } else if (f.populatedBy?.operations.has("UPDATE")) {
          set[f.property] = toStored(
            f,
            await plan.callback(f, "UPDATE", node, key, input),
          );
        }
      }
      prepared.push({ key, set, remove });
    }

    const actx = this.ctx();
    const adjustments = compileAdjust(actx, node, adjust);
    const stamped = timestamps(node, "UPDATE");
    // One statement per shape: which properties are removed.
    const shapes = new Map<string, typeof prepared>();
    for (const p of prepared) {
      const shape = p.remove.slice().sort().join("\0");
      const group = shapes.get(shape) ?? [];
      group.push(p);
      shapes.set(shape, group);
    }
    for (const [shape, group] of shapes) {
      const sctx = { ...actx, params: { ...actx.params } };
      const rowsParam = printExpr(
        bind(
          sctx,
          group.map((g) => ({ key: g.key, set: g.set })),
        ),
      );
      const sets = [
        "n += row.set",
        ...adjustments,
        ...stamped.map(([p, now]) => `n.${name(p)} = ${now}`),
      ];
      const removes = shape ? shape.split("\0") : [];
      await this.run(
        `UNWIND ${rowsParam} AS row\n` +
          `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = row.key\n` +
          `SET ${sets.join(", ")}` +
          (removes.length > 0
            ? `\nREMOVE ${removes.map((p) => `n.${name(p)}`).join(", ")}`
            : ""),
        sctx,
      );
    }
    const keys = todo.map((r) => r.key);
    this.info.nodesUpdated += keys.length;
    for (const key of keys) this.change.updated.push({ type: node.name, key });
    const entry = this.updated.get(node) ?? { keys: [], fields: new Set() };
    entry.keys.push(...keys);
    for (const f of written) entry.fields.add(f.name);
    this.updated.set(node, entry);
    return keys;
  }

  /**
   * A single relationship field allows one related node, whichever side
   * the relationship was created from. Check every node that gained one.
   */
  async checkCardinality(plan: WritePlan): Promise<void> {
    const model = this.env.model;
    interface Touched {
      field: RelationshipField;
      keys: unknown[];
      /** Links per key, for keys of nodes this plan creates. */
      fresh: Map<string, number>;
    }
    const touched = new Map<string, Touched>();
    // `type` is the node's concrete type: a node this plan creates that
    // gains exactly one link on the field cannot hold more than one.
    const add = (field: RelationshipField, key: unknown, type: string) => {
      const id = `${field.owner}.${field.name}`;
      const entry: Touched = touched.get(id) ?? {
        field,
        keys: [],
        fresh: new Map(),
      };
      entry.keys.push(key);
      if (plan.isFresh(type, key)) {
        const k = keyOf(key);
        entry.fresh.set(k, (entry.fresh.get(k) ?? 0) + 1);
      }
      touched.set(id, entry);
    };
    for (const link of plan.links) {
      if (!link.rel.list) add(declared(link.rel), link.from, link.rel.owner);
      for (const f of inverseFields(model, link.rel)) {
        if (!f.list) add(f, link.to, link.rel.target);
      }
    }
    for (const { field, keys: all, fresh } of touched.values()) {
      const keys = all.filter((k) => fresh.get(keyOf(k)) !== 1);
      if (keys.length === 0) continue;
      const owner = model.nodes.get(field.owner)!;
      const ctx = this.ctx();
      const text =
        `UNWIND ${printExpr(bind(ctx, dedupe(keys)))} AS k\n` +
        `MATCH (a:${name(owner.labels[0]!)}) WHERE a.${name(owner.key.property)} = k\n` +
        `WITH a, size([(a)${arrow(field, "", "x", undefined)} WHERE ${memberTest(model, "x", field)} | 1]) AS c WHERE c > 1\n` +
        `RETURN a.${name(owner.key.property)} AS key, c AS count`;
      const target = { name: field.target } as NodeType;
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

  /**
   * A required single relationship stays set: disconnecting from the other
   * side must not leave the owner without one.
   */
  async checkRequired(): Promise<void> {
    const model = this.env.model;
    const byField = new Map<RelationshipField, unknown[]>();
    for (const ref of this.disconnected) {
      const [ownerName, fieldName] = ref.field.split(".") as [string, string];
      const field = model.nodes
        .get(ownerName)!
        .fields.get(fieldName) as RelationshipField;
      // The concrete copy for the node that was disconnected.
      const rel =
        memberFields(model, field).find((m) => m.target === ref.to.type) ??
        field;
      if (!field.list && field.required) {
        byField.set(field, [...(byField.get(field) ?? []), ref.from.key]);
      }
      for (const f of inverseFields(model, rel)) {
        if (!f.list && f.required) {
          byField.set(f, [...(byField.get(f) ?? []), ref.to.key]);
        }
      }
    }
    for (const [field, keys] of byField) {
      const owner = model.nodes.get(field.owner)!;
      const ctx = this.ctx();
      const rows = await this.run(
        `UNWIND ${printExpr(bind(ctx, dedupe(keys)))} AS k\n` +
          `MATCH (a:${name(owner.labels[0]!)}) WHERE a.${name(owner.key.property)} = k\n` +
          `WITH a WHERE size([(a)${arrow(field, "", "x", undefined)} WHERE ${memberTest(model, "x", field)} | 1]) = 0\n` +
          `RETURN a.${name(owner.key.property)} AS key`,
        ctx,
      );
      if (rows.length > 0) {
        throw requiredMissing(owner, field, field.target, rows[0]!["key"]);
      }
    }
  }

  /** AFTER rules on created nodes, and on nodes this mutation updated. */
  async validateCreated(plan: WritePlan): Promise<void> {
    for (const [node, rows] of plan.creates) {
      const fields = new Set<string>();
      for (const f of node.fields.values()) {
        if (f.kind === "scalar" && f.authorization) fields.add(f.name);
      }
      await this.validateAfter(
        node,
        rows.map((r) => r.key),
        "CREATE",
        fields,
      );
    }
  }

  async validateUpdated(): Promise<void> {
    for (const [node, { keys, fields }] of this.updated) {
      await this.validateAfter(node, keys, "UPDATE", fields);
    }
  }

  /** AFTER rules for `op` on the nodes with `keys` (and written fields). */
  async validateAfter(
    node: NodeType,
    keys: unknown[],
    op: "CREATE" | "UPDATE",
    fields: Set<string>,
  ): Promise<void> {
    if (keys.length === 0) return;
    const ctx = this.ctx();
    const rule = and(
      authValidate(ctx, node, "n", op, "AFTER"),
      ...[...fields].map((f) => {
        const field = node.fields.get(f);
        return field
          ? fieldValidate(ctx, node, field, "n", op, "AFTER")
          : undefined;
      }),
    );
    if (!rule) return;
    const text =
      `UNWIND ${printExpr(bind(ctx, dedupe(keys)))} AS k\n` +
      `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = k\n` +
      `RETURN n.${name(node.key.property)} AS key, ${printExpr(rule)} AS ok`;
    const rows = await this.run(text, ctx);
    if (rows.some((r) => r["ok"] !== true)) throw forbidden(node, op);
  }

  /** Nodes by key, projected for the payload, inside the transaction. */
  async payload(
    node: NodeType,
    keys: unknown[],
    sets: SelectionSetNode[],
  ): Promise<unknown[]> {
    if (sets.length === 0) return keys.map(() => ({}));
    const ctx = this.ctx();
    const compiled = compileByKeys(ctx, node, keys, sets);
    const statement = compiled.statements[0]!;
    this.env.onStatement?.(statement);
    const result = await this.tx.execute(statement);
    return assertReadable(compiled.shape([result]) as unknown[]);
  }

  /**
   * Keys of the nodes `where` matches, under the filter rules for `op`.
   * More than `limit` is an error: bulk writes are bounded, not truncated.
   */
  async resolveKeys(
    node: NodeType,
    where: Input,
    op: "UPDATE" | "DELETE",
    limit: number,
  ): Promise<unknown[]> {
    const ctx = this.ctx();
    const pred = compileNodeWhere(ctx, node, "this", where);
    if (!pred) {
      throw requestError(
        "BAD_USER_INPUT",
        `\`where\` must filter something: bulk ${op.toLowerCase()}s never apply to every ${node.name} by accident`,
      );
    }
    const rows = await this.run(
      `MATCH (this:${name(node.labels[0]!)}) WHERE ${printExpr(pred)}` +
        andText(authFilter(ctx, node, "this", op)) +
        `\nRETURN this.${name(node.key.property)} AS key ORDER BY key LIMIT ${printExpr(bind(ctx, limit + 1))}`,
      ctx,
    );
    if (rows.length > limit) {
      throw requestError(
        "LIMIT_EXCEEDED",
        `more than ${limit} ${node.name} nodes match; narrow \`where\` or raise \`limit\``,
      );
    }
    return rows.map((r) => r["key"]);
  }

  /**
   * Delete nodes by key, with what `onDelete: CASCADE` reaches. Checks
   * BEFORE rules, RESTRICT, and required relationships of the survivors.
   */
  async delete(root: NodeType, keys: unknown[]): Promise<void> {
    const model = this.env.model;
    const doomed = new Map<NodeType, Map<string, unknown>>();
    let total = 0;
    let queue: Array<[NodeType, unknown[]]> = [[root, keys]];
    while (queue.length > 0) {
      const next: Array<[NodeType, unknown[]]> = [];
      for (const [node, batch] of queue) {
        const seen = doomed.get(node) ?? new Map<string, unknown>();
        const fresh = batch.filter((k) => !seen.has(keyOf(k)));
        if (fresh.length === 0) continue;
        checkAuthentication(this.ctx(), node, "DELETE");
        const ctx = this.ctx();
        const before = authValidate(ctx, node, "n", "DELETE", "BEFORE");
        const rows = await this.run(
          `UNWIND ${printExpr(bind(ctx, fresh))} AS k\n` +
            `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = k\n` +
            `RETURN n.${name(node.key.property)} AS key, ${before ? printExpr(before) : "true"} AS ok` +
            `, ${printExpr(authFilter(ctx, node, "n", "DELETE") ?? lit(true))} AS visible`,
          ctx,
        );
        for (const r of rows) {
          // Cascading into a node the caller may not delete is refused.
          if (r["ok"] !== true || r["visible"] !== true) {
            throw forbidden(node, "DELETE");
          }
          seen.set(keyOf(r["key"]), r["key"]);
        }
        doomed.set(node, seen);
        total += rows.length;
        if (total > this.env.maxBatch) {
          throw requestError(
            "LIMIT_EXCEEDED",
            `the delete reaches more than ${this.env.maxBatch} nodes through onDelete: CASCADE`,
          );
        }
        const found = rows.map((r) => r["key"]);
        for (const declaredField of node.fields.values()) {
          if (
            declaredField.kind !== "relationship" ||
            declaredField.onDelete !== "CASCADE"
          )
            continue;
          for (const f of memberFields(model, declaredField)) {
            const target = model.nodes.get(f.target)!;
            const c = this.ctx();
            const related = await this.run(
              `UNWIND ${printExpr(bind(c, found))} AS k\n` +
                seekThenExpand("n", node, "k", f, "", "m", target) +
                "\n" +
                `RETURN DISTINCT m.${name(target.key.property)} AS key`,
              c,
            );
            if (related.length > 0)
              next.push([target, related.map((r) => r["key"])]);
          }
        }
      }
      queue = next;
    }
    if (total === 0) return;

    const isDoomed = (type: string, key: unknown) =>
      !!model.nodes.get(type) &&
      doomed.get(model.nodes.get(type)!)?.has(keyOf(key)) === true;

    for (const [node, keysByNode] of doomed) {
      const nodeKeys = [...keysByNode.values()];
      // RESTRICT: related nodes must be gone first (or go in this delete).
      const restricting = [...node.fields.values()].flatMap((f) =>
        f.kind === "relationship" && f.onDelete === "RESTRICT"
          ? memberFields(model, f)
          : [],
      );
      for (const f of restricting) {
        const target = model.nodes.get(f.target)!;
        const ctx = this.ctx();
        const rows = await this.run(
          `UNWIND ${printExpr(bind(ctx, nodeKeys))} AS k\n` +
            seekThenExpand("n", node, "k", f, "", "m", target) +
            "\n" +
            `RETURN n.${name(node.key.property)} AS key, m.${name(target.key.property)} AS related`,
          ctx,
        );
        const blocking = rows.find((r) => !isDoomed(target.name, r["related"]));
        if (blocking) {
          throw requestError(
            "CONSTRAINT_VIOLATION",
            `${node.name} ${JSON.stringify(blocking["key"])} still has ${f.name} (onDelete: RESTRICT); remove them first`,
            undefined,
            { type: node.name, field: f.name },
          );
        }
      }
      // Required single relationships of surviving nodes that point here.
      for (const owner of model.nodes.values()) {
        for (const f of owner.fields.values()) {
          if (f.kind !== "relationship" || f.list || !f.required) continue;
          if (!f.members.includes(node.name)) continue;
          const ctx = this.ctx();
          const back = {
            ...f,
            direction: f.direction === "OUT" ? "IN" : "OUT",
          } as const;
          const rows = await this.run(
            `UNWIND ${printExpr(bind(ctx, nodeKeys))} AS k\n` +
              seekThenExpand("n", node, "k", back, "", "o", owner) +
              "\n" +
              `RETURN DISTINCT o.${name(owner.key.property)} AS key`,
            ctx,
          );
          const orphan = rows.find((r) => !isDoomed(owner.name, r["key"]));
          if (orphan) throw requiredMissing(owner, f, f.target, orphan["key"]);
        }
      }
    }

    const relIds = new Set<number>();
    for (const [node, keysByNode] of doomed) {
      const nodeKeys = [...keysByNode.values()];
      for (const key of nodeKeys) {
        this.disconnected.push(...(await this.neighbours(node, key)));
      }
      const ctx = this.ctx();
      const rows = await this.run(
        `UNWIND ${printExpr(bind(ctx, nodeKeys))} AS k\n` +
          `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = k\n` +
          `RETURN [(n)-[r]-() | id(r)] AS ids`,
        ctx,
      );
      for (const r of rows)
        for (const id of r["ids"] as number[]) relIds.add(id);
    }
    for (const [node, keysByNode] of doomed) {
      const nodeKeys = [...keysByNode.values()];
      const ctx = this.ctx();
      await this.run(
        `UNWIND ${printExpr(bind(ctx, nodeKeys))} AS k\n` +
          `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = k\n` +
          `DETACH DELETE n`,
        ctx,
      );
      this.info.nodesDeleted += nodeKeys.length;
      for (const key of nodeKeys) {
        this.change.deleted.push({ type: node.name, key });
      }
    }
    this.info.relationshipsDeleted += relIds.size;
  }

  /** The node's current relationships in the model, for the write-set. */
  async neighbours(node: NodeType, key: unknown): Promise<RelationshipRef[]> {
    // Relationship fields declared on this type, and those on other types
    // that point here: a relationship may be declared on either side.
    const model = this.env.model;
    const own = [...node.fields.values()].flatMap((f) =>
      f.kind === "relationship" ? memberFields(model, f) : [],
    );
    const inverse = [...model.nodes.values()].flatMap((n) =>
      [...n.fields.values()].filter(
        (f): f is RelationshipField =>
          f.kind === "relationship" && f.members.includes(node.name),
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

/** Fields on the other side of `rel`'s relationship type, pointing back. */
function inverseFields(
  model: GraphModel,
  rel: RelationshipField,
): RelationshipField[] {
  const target = model.nodes.get(rel.target)!;
  return [...target.fields.values()].filter(
    (f): f is RelationshipField =>
      f.kind === "relationship" &&
      f.type === rel.type &&
      f.members.includes(rel.owner) &&
      f.direction !== rel.direction,
  );
}

/** `x:A OR x:B`: the field's target types (one, or an abstract's members). */
function memberTest(
  model: GraphModel,
  variable: string,
  field: RelationshipField,
): string {
  return field.members
    .map((m) => labelTest(variable, model.nodes.get(m)!))
    .join(" OR ");
}

/** `n.p = <expr>` assignments for an `adjust` input. */
function compileAdjust(
  ctx: CompileContext,
  node: NodeType,
  adjust: Input | undefined,
): string[] {
  const out: string[] = [];
  for (const [fieldName, ops] of Object.entries(adjust ?? {})) {
    if (ops === null || ops === undefined) continue;
    const f = node.fields.get(fieldName);
    if (f?.kind !== "scalar") continue;
    const entries = Object.entries(ops as Input).filter(
      ([, x]) => x !== null && x !== undefined,
    );
    if (entries.length !== 1) {
      throw requestError(
        "BAD_USER_INPUT",
        `adjust.${fieldName} takes exactly one operation`,
      );
    }
    const [op, value] = entries[0]!;
    const target = prop(v("n"), f.property);
    let expr: Expr;
    if (f.list) {
      const current = fn("coalesce", target, { kind: "list", items: [] });
      if (op === "push") {
        expr = bin("+", current, bind(ctx, value));
      } else if (op === "pop") {
        if (!Number.isInteger(value) || (value as number) < 0) {
          throw requestError(
            "BAD_USER_INPUT",
            `adjust.${fieldName}.pop must be a non-negative integer`,
          );
        }
        // `l[..-n]` is broken in LoraDB 0.15; size arithmetic is not.
        expr = {
          kind: "slice",
          target: current,
          from: undefined,
          to: bin("-", fn("size", current), bind(ctx, value)),
        };
      } else {
        const x = "adjust_item";
        expr = {
          kind: "listComprehension",
          variable: x,
          list: current,
          where: { kind: "not", expr: bin("IN", v(x), bind(ctx, value)) },
          projection: undefined,
        };
      }
    } else {
      const current = fn("coalesce", target, lit(0));
      const operand = bind(ctx, value);
      if (op === "divide" && Number(value) === 0) {
        throw requestError(
          "BAD_USER_INPUT",
          `adjust.${fieldName}.divide by zero`,
        );
      }
      const opMap: Record<string, "+" | "-" | "*" | "/"> = {
        add: "+",
        subtract: "-",
        multiply: "*",
        divide: "/",
      };
      expr = bin(opMap[op]!, current, operand);
      // LoraDB divides integers as floats; keep Int fields integral.
      if (op === "divide" && f.type === "Int") expr = fn("toInteger", expr);
    }
    out.push(`n.${name(f.property)} = ${printExpr(expr)}`);
  }
  return out;
}

function notConnected(
  owner: NodeType,
  rel: RelationshipField,
  target: NodeType,
  to: unknown,
) {
  return requestError(
    "NOT_FOUND",
    `${owner.name}.${rel.name}: ${to === undefined ? `no ${target.name} is connected` : `${target.name} ${JSON.stringify(to)} is not connected`}`,
    undefined,
    { type: target.name },
  );
}

function requiredMissing(
  owner: NodeType,
  field: RelationshipField,
  target: string,
  key: unknown,
) {
  return requestError(
    "CONSTRAINT_VIOLATION",
    `${owner.name} ${JSON.stringify(key)} requires a ${target} (${owner.name}.${field.name})`,
    undefined,
    { type: owner.name, field: field.name },
  );
}

function dedupe(keys: unknown[]): unknown[] {
  return [...new Map(keys.map((k) => [keyOf(k), k])).values()];
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
    requestContext: env.requestContext,
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

  const tx =
    owned ??
    (await env.driver.begin!({
      mode: "write",
      timeoutMs: env.timeoutMs,
      signal: env.signal,
    }));
  const change = emptyChange(op, fieldName);
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
            andText(authFilter(ctx, node, "n", "DELETE")) +
            `\nRETURN n.${name(node.key.property)} AS key`,
          ctx,
        );
        await runner.delete(
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
        await runner.delete(node, keys);
        payload = runner.info;
        break;
      }
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
  const keys = inputs.map((i) => i[node.key.name]);
  if (new Set(keys.map(keyOf)).size < keys.length) {
    throw requestError(
      "CONSTRAINT_VIOLATION",
      `the input names a ${node.name} ${node.key.name} twice`,
      undefined,
      { type: node.name, field: node.key.name },
    );
  }
  // Existing nodes the caller may update. A hidden one looks new, and its
  // create then fails on the key constraint like any taken key would.
  const ctx = runner.ctx();
  const existing = new Set(
    (
      await runner.run(
        `UNWIND ${printExpr(bind(ctx, keys))} AS k\n` +
          `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = k` +
          andText(authFilter(ctx, node, "n", "UPDATE")) +
          `\nRETURN n.${name(node.key.property)} AS key`,
        ctx,
      )
    ).map((r) => keyOf(r["key"])),
  );
  const updates: Array<{ key: unknown; input: Input }> = [];
  for (const [i, input] of inputs.entries()) {
    if (existing.has(keyOf(keys[i]))) {
      // Fields set only on create keep the value they were created with.
      const kept = Object.fromEntries(
        Object.entries(input).filter(([k]) => {
          const f = node.fields.get(k);
          return f?.kind !== "scalar" || settable(f, "UPDATE");
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
