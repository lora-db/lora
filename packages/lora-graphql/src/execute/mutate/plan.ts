// The write plan: one mutation's input turned into the creates, links,
// disconnects and nested updates to run, with the input checks that need
// no statement (required fields, settable fields, @authentication).

import {
  checkAuthentication,
  checkFieldAuthentication,
  checkKeyScope,
  checkPropertyAccess,
  propertyAccess,
} from "../../compile/auth.js";
import type { CompileContext } from "../../compile/context.js";
import { keyOf } from "../../compile/read/by-keys.js";
import { requestError } from "../../errors.js";
import { toStored } from "../../model/points.js";
import { memberFields } from "../../model/relations.js";
import type {
  GraphModel,
  NodeType,
  RelationshipField,
  ScalarField,
} from "../../model/types.js";
import { requiredOnCreate, settable } from "../../schema/mutations.js";
import type { MutationEnv } from "./env.js";
import { inverseFields } from "./fragments.js";

export type Input = Record<string, unknown>;

export interface Link {
  rel: RelationshipField;
  /** @key of the node declaring the field. */
  from: unknown;
  /** @key of the target node. */
  to: unknown;
  /** Relationship properties the input sets. */
  props: Record<string, unknown>;
  /** `@default`s of the properties it leaves out: for a new relationship only. */
  defaults: Record<string, unknown>;
  /**
   * Relationship properties the input sets whose rules refuse it when the
   * relationship is new (`create`) or already exists (`update`): known
   * only once the MERGE ran, so the check follows it (and rolls back).
   */
  refused?: { create?: Error; update?: Error };
}

export interface CreateRow {
  key: unknown;
  props: Input;
  /**
   * Fields the input sets. Field rules guard these only: a `@default`,
   * `@populatedBy` value or generated key is the server's, not the
   * caller's write (G-22).
   */
  written: Set<string>;
}

export interface EdgeUpdate {
  rel: RelationshipField;
  from: unknown;
  /** Target key; undefined for a single relationship's current target. */
  to: unknown;
  set: Input;
  remove: string[];
}

export interface NodeUpdate {
  rel: RelationshipField;
  from: unknown;
  to: unknown;
  input: Input;
}

const asList = <T>(x: unknown): T[] =>
  x === null || x === undefined ? [] : Array.isArray(x) ? (x as T[]) : [x as T];

/** Writes planned from one mutation's input. */
export class WritePlan {
  creates = new Map<NodeType, CreateRow[]>();
  links: Link[] = [];
  disconnects: Array<{
    rel: RelationshipField;
    from: unknown;
    /** Keys to disconnect; undefined: every current target (single fields). */
    to: unknown[] | undefined;
    /**
     * Replacing a single relationship: the target being connected, whose
     * relationship (and its properties) stays if it is already there.
     */
    keep?: unknown;
  }> = [];
  edgeUpdates: EdgeUpdate[] = [];
  nodeUpdates: NodeUpdate[] = [];
  /** `update: { rel: { delete } }`: connected nodes to delete. */
  nestedDeletes: Array<{
    rel: RelationshipField;
    from: unknown;
    where: Input | undefined;
    limit: number | undefined;
  }> = [];
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
    checkKeyScope(this.ctx, node, key, this.env.viewerKey);
    const props: Input = {};
    const written = new Set<string>();
    if (input[node.key.name] !== undefined && input[node.key.name] !== null) {
      written.add(node.key.name);
    }
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
        written.add(f.name);
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
    list.push({ key, props, written });
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
      if (value && !settable(f, "CREATE")) {
        throw requestError(
          "BAD_USER_INPUT",
          `${node.name}.${f.name} cannot be set on create`,
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
    const value = await cb({
      operation,
      type: node.name,
      field: f.name,
      key,
      input,
      context: this.env.requestContext,
    });
    // A required field must stay set, whatever the callback returns.
    if (f.required && (value === null || value === undefined)) {
      throw requestError(
        "CONSTRAINT_VIOLATION",
        `${node.name}.${f.name} is required, but its @populatedBy callback ${f.populatedBy!.callback} returned ${String(value)}`,
        undefined,
        { type: node.name, field: f.name },
      );
    }
    return value;
  }

  /** Plan connect / create / update / disconnect under one relationship field. */
  relate(
    owner: NodeType,
    ownerKey: unknown,
    rel: RelationshipField,
    value: Input,
    update: boolean,
  ): void {
    // `@authentication` on the relationship field covers writing it too.
    checkFieldAuthentication(
      this.ctx,
      owner.name,
      rel,
      update ? "UPDATE" : "CREATE",
    );
    if (asList(value["connect"]).length + asList(value["create"]).length > 0) {
      checkFieldAuthentication(
        this.ctx,
        owner.name,
        rel,
        "CREATE_RELATIONSHIP",
      );
    }
    if (
      (value["disconnect"] != null && value["disconnect"] !== false) ||
      (value["delete"] != null && value["delete"] !== false)
    ) {
      checkFieldAuthentication(
        this.ctx,
        owner.name,
        rel,
        "DELETE_RELATIONSHIP",
      );
    }
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
    const nestedDelete = value["delete"] as Input | boolean | null | undefined;
    if (update && nestedDelete != null && nestedDelete !== false) {
      checkAuthentication(this.ctx, target, "DELETE");
      const spec = nestedDelete === true ? {} : nestedDelete;
      this.nestedDeletes.push({
        rel,
        from: ownerKey,
        where: (spec["where"] as Input | null | undefined) ?? undefined,
        limit: (spec["limit"] as number | null | undefined) ?? undefined,
      });
    }
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
        // A single relationship is replaced, never doubled. Re-connecting
        // the current target keeps its relationship as it is.
        const keep =
          connect.length === 1 ? connect[0]![target.key.name] : undefined;
        this.disconnects.push({
          rel,
          from: ownerKey,
          to: undefined,
          ...(keep !== undefined ? { keep } : {}),
        });
      }
    }
    for (const c of connect) {
      this.links.push({
        rel,
        from: ownerKey,
        to: c[target.key.name],
        ...edgeProps(this.ctx, rel, c["edge"] as Input | undefined),
      });
    }
    for (const c of create) {
      const key = this.create(target, c["node"] as Input, rel);
      this.links.push({
        rel,
        from: ownerKey,
        to: key,
        ...edgeProps(this.ctx, rel, c["edge"] as Input | undefined),
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
          checkPropertyAccess(this.ctx, props.name, f, "UPDATE");
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
  ctx: CompileContext,
  rel: RelationshipField,
  edge: Input | undefined,
): Pick<Link, "props" | "defaults" | "refused"> {
  if (!rel.properties) return { props: {}, defaults: {} };
  const props = ctx.model.relationshipProperties.get(rel.properties)!;
  const out: Record<string, unknown> = {};
  const defaults: Record<string, unknown> = {};
  let refused: Link["refused"];
  for (const f of props.fields.values()) {
    const value = edge?.[f.name];
    if (value !== undefined && value !== null && settable(f, "CREATE")) {
      // Setting a property on connect creates it on a new relationship
      // and updates it on an existing one: both rules must be known.
      const onCreate = propertyAccess(ctx, f, "CREATE");
      const onUpdate = propertyAccess(ctx, f, "UPDATE");
      if (onCreate !== "allowed" && onUpdate !== "allowed") {
        checkPropertyAccess(ctx, props.name, f, "CREATE");
      }
      if (onCreate !== "allowed") {
        refused = {
          ...refused,
          create: propertyError(props.name, f, "CREATE", onCreate),
        };
      }
      if (onUpdate !== "allowed") {
        refused = {
          ...refused,
          update: propertyError(props.name, f, "UPDATE", onUpdate),
        };
      } else if (!settable(f, "UPDATE")) {
        // Re-connecting an existing pair would update the property.
        refused = {
          ...refused,
          update: requestError(
            "BAD_USER_INPUT",
            `${props.name}.${f.name} is set when the relationship is created; it cannot change on a re-connect`,
          ),
        };
      }
      out[f.property] = toStored(f, value);
    } else if (f.defaultValue) {
      defaults[f.property] = storedDefault(f);
    }
  }
  return refused ? { props: out, defaults, refused } : { props: out, defaults };
}

function propertyError(
  type: string,
  f: ScalarField,
  op: "CREATE" | "UPDATE",
  access: "forbidden" | "unauthenticated" | "allowed",
): Error {
  return access === "unauthenticated"
    ? requestError(
        "UNAUTHENTICATED",
        `${type}.${f.name} needs an authenticated request`,
      )
    : requestError(
        "FORBIDDEN",
        `not allowed to ${op.toLowerCase()} ${type}.${f.name}`,
      );
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

export function assertUniqueKeys(plan: WritePlan): void {
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
