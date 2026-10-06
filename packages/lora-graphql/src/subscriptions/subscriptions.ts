// Subscriptions: the subscribers of each node type, the delivery of every
// committed change to those it concerns, and the checks that keep an
// event from reaching a subscriber who may not read its node (visibility
// after the write, deletions inside the deleting transaction, the related
// node of a CONNECT / DISCONNECT).

import type { FieldNode, GraphQLResolveInfo, GraphQLSchema } from "graphql";
import {
  authFilter,
  authValidate,
  checkAuthentication,
  checkFieldAuthentication,
  fieldReadGuard,
} from "../compile/auth.js";
import { stableKey } from "../compile/cache.js";
import { bind, type CompileContext } from "../compile/context.js";
import {
  and,
  bin,
  fn,
  lit,
  printClauses,
  prop,
  v,
  type Expr,
} from "../compile/cypher.js";
import { compileNodeWhere } from "../compile/filter.js";
import { compileRoot } from "../compile/read/root.js";
import { keyOf } from "../compile/read/by-keys.js";
import { type CompiledRead } from "../compile/read/types.js";
import {
  coercedVariables,
  type SelectionContext,
} from "../compile/selection.js";
import type { LoraDriver, QueryResult, Statement } from "../driver.js";
import { requestError } from "../errors.js";
import type { WriteChange } from "../execute/changes.js";
import type { GraphModel, NodeType } from "../model/types.js";
import type { LoraGraphQLContext } from "../options.js";
import { infoContext } from "../runtime/request.js";
import type { ChangeEvent } from "../schema/hooks.js";
import type { ChangeHub } from "./change-hub.js";
import {
  sameContextReads,
  slotCompile,
  type SubscriptionCompile,
} from "./compiles.js";
import { CHANGE, changeEvents, SELF_OWNS } from "./events.js";
import { filterCost } from "./filter-cost.js";

/** A subscriber's share of one change: the events of its node type. */
interface SubscriberDelivery {
  change: WriteChange;
  events: ChangeEvent[];
}
type SubscriberSink = (delivery: SubscriberDelivery) => void;

/** A subscription's visibility check (see `#visibleCheck`). */
interface VisibilityCheck {
  statement(keys: unknown[]): Statement;
  settled(): boolean;
  run(keys: unknown[], change: object): Promise<Set<string>>;
}

/** A subscriber whose DELETE events are checked inside the transaction. */
interface DeleteProbe {
  check: VisibilityCheck;
}

interface SubscriberGroup {
  node: NodeType;
  all: Set<SubscriberSink>;
  byKey: Map<string, Set<SubscriberSink>>;
}

/**
 * What subscriptions need of the `LoraGraphQL` instance that owns them:
 * its limits, and the request machinery they share with queries.
 */
export interface SubscriptionHost {
  model: GraphModel;
  driver: LoraDriver;
  changes: ChangeHub;
  /** The `maxSubscriptions` option, defaulted. */
  maxSubscriptions: number;
  /** The `subscriptionScope` option, defaulted. */
  subscriptionScope: (context: unknown) => object | undefined;
  /** The `maxSubscriptionFilterDepth` option, defaulted. */
  maxSubscriptionFilterDepth: number;
  /** The `subscriptionTimeoutMs` option, defaulted. */
  subscriptionTimeoutMs: number;
  /** The executable schema. */
  schema(): GraphQLSchema;
  /** The request's verified claims. */
  jwt(context: unknown): Record<string, unknown> | undefined;
  /** A compile context for the request. */
  context(base: SelectionContext, context: unknown): CompileContext;
  /** Measured maximum degrees, by `Owner.field`. */
  degrees(): ReadonlyMap<string, number>;
  /** Bumped when statistics change. */
  statisticsVersion(): number;
  /** Charge `cost` to the operation; COST_EXCEEDED past its limit. */
  charge(field: string, cost: number, context: unknown): void;
  /** Run a compiled read as the root field `field`. */
  run(
    field: string,
    compiled: CompiledRead,
    context: unknown,
    info?: GraphQLResolveInfo,
    change?: object,
    shareKey?: string,
  ): Promise<unknown>;
}

export class Subscriptions {
  readonly #host: SubscriptionHost;
  readonly #model: GraphModel;
  readonly #driver: LoraDriver;
  readonly #changes: ChangeHub;
  readonly #maxSubscriptions: number;
  readonly #subscriptionScope: (context: unknown) => object | undefined;
  readonly #maxSubscriptionFilterDepth: number;
  readonly #subscriptionTimeoutMs: number;
  /** Live subscriptions per scope. */
  readonly #subscriptionCounts = new WeakMap<object, number>();
  /**
   * Subscribers by node type, then unfiltered or by key: a change reaches
   * only the subscribers of the types it touches, with its events built
   * once per type.
   */
  readonly #subscribers = new Map<string, SubscriberGroup>();
  /** Subscribers following a key whose deletions need a check. */
  readonly #probes = new WeakMap<SubscriberSink, DeleteProbe>();
  /** Per change: the doomed keys each probed subscriber could read. */
  readonly #deleteSeen = new WeakMap<
    WriteChange,
    Map<DeleteProbe, Set<string>>
  >();
  #undispatch: (() => unknown) | undefined;
  /** Changed-node reads compiled per subscription context and field node. */
  readonly #byKeyCompiles = new WeakMap<
    object,
    WeakMap<FieldNode, SubscriptionCompile<CompiledRead>>
  >();
  /**
   * Reads made for subscribers of one change, by statement text and
   * parameters: subscribers whose checks or node reads compile to the same
   * statement share one database call.
   */
  readonly #sharedReads = new WeakMap<
    object,
    Map<string, Promise<QueryResult[]>>
  >();

  constructor(host: SubscriptionHost) {
    this.#host = host;
    this.#model = host.model;
    this.#driver = host.driver;
    this.#changes = host.changes;
    this.#maxSubscriptions = host.maxSubscriptions;
    this.#subscriptionScope = host.subscriptionScope;
    this.#maxSubscriptionFilterDepth = host.maxSubscriptionFilterDepth;
    this.#subscriptionTimeoutMs = host.subscriptionTimeoutMs;
  }

  /**
   * Events for one node type, from the exact write-sets of mutations made
   * through this instance. Nodes the subscriber may not read are skipped;
   * deletions of rule-protected types are only sent to subscribers that
   * follow that key and could read the node before it went (checked in
   * the deleting transaction).
   */
  subscribe(
    node: NodeType,
    args: Record<string, unknown>,
    context: unknown,
  ): AsyncIterableIterator<ChangeEvent> {
    const base: SelectionContext = {
      schema: this.#host.schema(),
      fragments: {},
      variables: {},
    };
    // Checked now, so an unauthenticated subscribe fails as a result
    // rather than as a broken stream.
    const ctx = this.#host.context(base, context);
    checkAuthentication(ctx, node, "SUBSCRIBE");
    checkAuthentication(ctx, node, "READ");
    // Rules the claims alone decide against fail here too.
    for (const op of ["SUBSCRIBE", "READ"] as const) {
      authFilter(ctx, node, "n", op);
      authValidate(ctx, node, "n", op, "BEFORE");
    }
    // `where` runs on every change: bound its nesting, charge it once.
    const field = `${node.name[0]!.toLowerCase()}${node.name.slice(1)}Changed`;
    const where = args["where"] as Record<string, unknown> | null | undefined;
    this.#host.charge(
      field,
      1 +
        filterCost(
          {
            model: this.#model,
            degrees: this.#host.degrees(),
            maxDepth: this.#maxSubscriptionFilterDepth,
          },
          node,
          where,
          0,
        ),
      context,
    );
    const scope = this.#subscriptionScope(context);
    const live = scope ? (this.#subscriptionCounts.get(scope) ?? 0) : 0;
    if (scope && live >= this.#maxSubscriptions) {
      throw requestError(
        "LIMIT_EXCEEDED",
        `at most ${this.#maxSubscriptions} subscriptions at a time`,
      );
    }
    if (scope) this.#subscriptionCounts.set(scope, live + 1);
    let counted = !!scope;
    const release = () => {
      if (!counted || !scope) return;
      counted = false;
      const n = (this.#subscriptionCounts.get(scope) ?? 1) - 1;
      if (n > 0) this.#subscriptionCounts.set(scope, n);
      else this.#subscriptionCounts.delete(scope);
    };
    // The stream's own signal: ended by `context.signal` or by return().
    // An async generator runs return() only once its pending next()
    // yields, and a quiet or filtered stream may never yield again, so
    // return() aborts the changes() underneath, which wakes that next().
    const stop = new AbortController();
    const outer = (context as LoraGraphQLContext | undefined)?.signal;
    const end = () => {
      outer?.removeEventListener("abort", end);
      stop.abort();
      release();
    };
    if (outer?.aborted) end();
    else outer?.addEventListener("abort", end, { once: true });
    const events = this.#events(node, args, context, base, stop.signal);
    const iterator: AsyncIterableIterator<ChangeEvent> = {
      [Symbol.asyncIterator]: () => iterator,
      next: async () => {
        const result = await events.next();
        if (result.done) end();
        return result;
      },
      return: (value?: unknown) => {
        end();
        return events.return(value);
      },
      throw: (error?: unknown) => {
        end();
        return events.throw(error);
      },
    };
    return iterator;
  }

  async *#events(
    node: NodeType,
    args: Record<string, unknown>,
    context: unknown,
    base: SelectionContext,
    signal: AbortSignal,
  ): AsyncGenerator<ChangeEvent> {
    const key = args[node.key.name];
    const where = args["where"] as Record<string, unknown> | null | undefined;
    const offered = new Set<string>(node.subscriptions);
    if (node.subscriptionOptions.relationships) {
      offered.add("CONNECT");
      offered.add("DISCONNECT");
    }
    const wanted = new Set(
      ((args["operations"] as string[] | null) ?? [...offered]).filter((op) =>
        offered.has(op),
      ),
    );
    const ruled = (r: { operations: ReadonlySet<string> }) =>
      r.operations.has("READ") || r.operations.has("SUBSCRIBE");
    const guarded =
      !!node.authorization?.filter.some(ruled) ||
      !!node.authorization?.validate.some(ruled);
    // Whether events must be checked in the database: read rules, or a
    // `where` (on the node as it is after the write).
    const check = guarded || (where != null && Object.keys(where).length > 0);
    const visible = check
      ? this.#visibleCheck(node, where, base, context)
      : undefined;
    // Only changes to this type (and this key, when one is followed)
    // arrive, with their events already built.
    const related = (events: ChangeEvent[]) =>
      this.#relatedVisible(node, events, base, context, signal);
    // A deleted node cannot be checked after the fact: a follower of its
    // key without a `where` is checked inside the deleting transaction,
    // on the node as it was; other checked subscribers get no deletions.
    const probe: DeleteProbe | undefined =
      visible && key != null && !where && wanted.has("DELETE")
        ? { check: visible }
        : undefined;
    const deliveries = this.#changes.queue<SubscriberDelivery>(
      (sink) =>
        this.#addSubscriber(
          node,
          key == null ? undefined : keyOf(key),
          sink,
          probe,
        ),
      { signal },
    );
    for await (const { change, events: all } of deliveries) {
      const events = all.filter((e) => wanted.has(e.operation));
      if (events.length === 0) continue;
      const probed = probe && this.#deleteSeen.get(change)?.get(probe);
      const deletions = events.filter(
        (e) =>
          e.operation === "DELETE" &&
          (!check ||
            (probe !== undefined &&
              (probed?.has(keyOf(e.key)) ?? probe.check.settled()))),
      );
      const live = events.filter((e) => e.operation !== "DELETE");
      const seen = visible
        ? await visible.run(
            live.map((e) => e.key),
            change,
          )
        : undefined;
      const relationships = live.filter(
        (e) =>
          e.relationship !== undefined && (!seen || seen.has(keyOf(e.key))),
      );
      const named =
        relationships.length > 0
          ? await related(relationships)
          : new Set<ChangeEvent>();
      for (const event of [...live, ...deletions]) {
        if (
          event.operation !== "DELETE" &&
          seen &&
          !seen.has(keyOf(event.key))
        ) {
          continue;
        }
        if (event.relationship && !named.has(event)) continue;
        // Events are built once per change and type: each subscriber gets
        // its own copy, since per-event cost is charged by root value.
        const own = { ...event };
        Object.defineProperty(own, CHANGE, { value: change });
        yield own;
      }
    }
  }

  /** Index a subscriber by type and key; returns its removal. */
  #addSubscriber(
    node: NodeType,
    key: string | undefined,
    sink: SubscriberSink,
    probe?: DeleteProbe,
  ): () => void {
    if (probe) this.#probes.set(sink, probe);
    // One listener serves every subscriber, registered while any exist.
    if (!this.#undispatch) {
      const dispatch = (change: WriteChange) => this.#dispatch(change);
      this.#undispatch = this.#changes.listen(dispatch);
    }
    let group = this.#subscribers.get(node.name);
    if (!group) {
      group = { node, all: new Set(), byKey: new Map() };
      this.#subscribers.set(node.name, group);
    }
    const g = group;
    let sinks = g.all;
    if (key !== undefined) {
      sinks = g.byKey.get(key) ?? new Set();
      g.byKey.set(key, sinks);
    }
    sinks.add(sink);
    return () => {
      sinks.delete(sink);
      if (key !== undefined && sinks.size === 0) g.byKey.delete(key);
      if (
        g.all.size === 0 &&
        g.byKey.size === 0 &&
        this.#subscribers.get(node.name) === g
      ) {
        this.#subscribers.delete(node.name);
        if (this.#subscribers.size === 0) {
          this.#undispatch?.();
          this.#undispatch = undefined;
        }
      }
    };
  }

  /**
   * Hand one change to the subscribers of the types it touches: each
   * type's events are built once, and keyed subscribers get only the
   * events of their key.
   */
  #dispatch(change: WriteChange): void {
    if (this.#subscribers.size === 0) return;
    const types = new Set<string>();
    for (const refs of [change.deleted, change.created, change.entities]) {
      for (const e of refs) types.add(e.type);
    }
    for (const refs of [change.connected, change.disconnected]) {
      for (const r of refs) {
        types.add(r.from.type);
        types.add(r.to.type);
      }
    }
    const deliver = (sinks: Set<SubscriberSink>, events: ChangeEvent[]) => {
      for (const sink of sinks) {
        try {
          sink({ change, events });
        } catch {
          // One subscriber's failure must not stop the others.
        }
      }
    };
    for (const type of types) {
      const group = this.#subscribers.get(type);
      if (!group) continue;
      const events = changeEvents(change, group.node);
      if (events.length === 0) continue;
      if (group.all.size > 0) deliver(group.all, events);
      if (group.byKey.size === 0) continue;
      const byKey = new Map<string, ChangeEvent[]>();
      for (const e of events) {
        const id = keyOf(e.key);
        if (!group.byKey.has(id)) continue;
        const list = byKey.get(id);
        if (list) list.push(e);
        else byKey.set(id, [e]);
      }
      for (const [id, list] of byKey) {
        const sinks = group.byKey.get(id);
        if (sinks) deliver(sinks, list);
      }
    }
  }

  /**
   * Whether a subscription may see nodes: which of `keys` it may read
   * (and its `where` matches), in one query per change. The statement is
   * compiled once per subscription, again only when its claims or the
   * `$context` values it read change. `statement` gives the check for
   * other uses (a deletion is checked inside its transaction); `settled`
   * says whether the claims alone grant reading every node of the type.
   */
  #visibleCheck(
    node: NodeType,
    where: Record<string, unknown> | null | undefined,
    base: SelectionContext,
    context: unknown,
  ): VisibilityCheck {
    let cached:
      | (SubscriptionCompile<Statement> & { settled: boolean })
      | undefined;
    const compile = () => {
      const claims = stableKey(this.#host.jwt(context) ?? null);
      let entry = cached;
      if (
        !entry ||
        claims === undefined ||
        entry.claims !== claims ||
        !sameContextReads(entry.contextReads, context)
      ) {
        const ctx = this.#host.context(base, context);
        const slot: unknown[] = [];
        const rules = and(
          authFilter(ctx, node, "n", "READ"),
          authValidate(ctx, node, "n", "READ", "BEFORE"),
          authFilter(ctx, node, "n", "SUBSCRIBE"),
          authValidate(ctx, node, "n", "SUBSCRIBE", "BEFORE"),
        );
        const text = printClauses([
          { kind: "unwind", expr: bind(ctx, slot), alias: "k" },
          {
            kind: "match",
            pattern: {
              start: { variable: "n", labels: [node.labels[0]!] },
              hops: [],
            },
            where: and(
              bin("=", prop(v("n"), node.key.property), v("k")),
              compileNodeWhere(ctx, node, "n", where),
              rules,
            ),
          },
          {
            kind: "return",
            items: [{ expr: prop(v("n"), node.key.property), alias: "key" }],
          },
        ]);
        // The key list is bound directly, so the slot is always found.
        entry = {
          ...slotCompile(
            { text, params: ctx.params },
            slot,
            claims,
            0,
            undefined,
            ctx.contextReads,
          )!,
          settled: rules === undefined,
        };
        cached = claims === undefined ? undefined : entry;
      }
      return entry;
    };
    const statement = (keys: unknown[]) => {
      const entry = compile();
      return {
        statement: {
          text: entry.compiled.text,
          params: { ...entry.compiled.params, [entry.param]: keys },
        },
        share:
          entry.share === undefined
            ? undefined
            : `${entry.share}\0${keys.map(keyOf).join("\0")}`,
      };
    };
    return {
      statement: (keys) => statement(keys).statement,
      settled: () => {
        try {
          return compile().settled;
        } catch {
          return false;
        }
      },
      run: async (keys, change) => {
        if (keys.length === 0) return new Set();
        const { statement: s, share } = statement(keys);
        const statements = [s];
        const [result] = await this.shared(
          change,
          statements,
          () =>
            this.#driver.run(statements, {
              mode: "read",
              timeoutMs: this.#subscriptionTimeoutMs,
              verified: true,
            }),
          share,
        );
        return new Set(result!.rows.map((r) => keyOf(r["key"])));
      },
    };
  }

  /**
   * The CONNECT / DISCONNECT events whose related node the subscriber may
   * read (its type's READ rules) through a field it may read (the
   * declaring field's READ rules), checked after the write. The others are
   * dropped, so an event never names a node the subscriber could not
   * read; a related node that is gone, which cannot be checked, drops the
   * event unless the claims alone settle every rule.
   */
  async #relatedVisible(
    node: NodeType,
    events: ChangeEvent[],
    base: SelectionContext,
    context: unknown,
    signal: AbortSignal,
  ): Promise<Set<ChangeEvent>> {
    type Group =
      | { kind: "deny" }
      | { kind: "allow"; events: ChangeEvent[] }
      | {
          kind: "check";
          statement: Statement;
          slot: string;
          events: ChangeEvent[];
        };
    const groups = new Map<string, Group>();
    for (const event of events) {
      const rel = event.relationship!;
      const selfOwns = (event as { [SELF_OWNS]?: boolean })[SELF_OWNS] ?? true;
      const id = `${rel.field}\0${rel.relatedType}\0${selfOwns}`;
      const known = groups.get(id);
      if (known) {
        if (known.kind !== "deny") known.events.push(event);
        continue;
      }
      groups.set(
        id,
        this.#relatedGroup(node, rel, selfOwns, base, context, event),
      );
    }
    const out = new Set<ChangeEvent>();
    const checks: Array<Extract<Group, { kind: "check" }>> = [];
    for (const g of groups.values()) {
      if (g.kind === "allow") for (const e of g.events) out.add(e);
      else if (g.kind === "check") checks.push(g);
    }
    if (checks.length === 0) return out;
    const results = await this.#driver.run(
      checks.map((g) => ({
        text: g.statement.text,
        params: {
          ...g.statement.params,
          [g.slot]: g.events.map((e) => ({
            i: events.indexOf(e),
            s: e.key,
            r: e.relationship!.relatedKey,
          })),
        },
      })),
      {
        mode: "read",
        timeoutMs: this.#subscriptionTimeoutMs,
        signal,
        verified: true,
      },
    );
    for (const result of results) {
      for (const row of result.rows) {
        const event = events[row["i"] as number];
        if (event) out.add(event);
      }
    }
    return out;
  }

  /** How one field and related type's relationship events are checked. */
  #relatedGroup(
    node: NodeType,
    rel: NonNullable<ChangeEvent["relationship"]>,
    selfOwns: boolean,
    base: SelectionContext,
    context: unknown,
    first: ChangeEvent,
  ):
    | { kind: "deny" }
    | { kind: "allow"; events: ChangeEvent[] }
    | {
        kind: "check";
        statement: Statement;
        slot: string;
        events: ChangeEvent[];
      } {
    const related = this.#model.nodes.get(rel.relatedType);
    if (!related) return { kind: "deny" };
    const [ownerName, fieldName] = rel.field.split(".");
    const owner = ownerName ? this.#model.nodes.get(ownerName) : undefined;
    const field = fieldName ? owner?.fields.get(fieldName) : undefined;
    const ctx = this.#host.context(base, context);
    let condition: Expr | undefined;
    let selfGuard: Expr | undefined;
    try {
      checkAuthentication(ctx, related, "READ");
      condition = and(
        authFilter(ctx, related, "m", "READ"),
        authValidate(ctx, related, "m", "READ", "BEFORE"),
      );
      if (owner && field?.kind === "relationship") {
        checkFieldAuthentication(ctx, owner.name, field);
        const guard = fieldReadGuard(ctx, owner, field, selfOwns ? "n" : "m");
        const when = guard && fn("coalesce", guard.when, lit(false));
        if (selfOwns) selfGuard = when;
        else condition = and(condition, when);
      }
    } catch {
      // The claims alone refuse: the events are not sent.
      return { kind: "deny" };
    }
    if (condition === undefined && selfGuard === undefined) {
      // Nothing depends on the nodes: no check needed.
      return { kind: "allow", events: [first] };
    }
    const slot: unknown[] = [];
    const pairs = bind(ctx, slot);
    const match = (
      variable: string,
      type: NodeType,
      end: "r" | "s",
      where: Expr | undefined,
    ) => ({
      kind: "match" as const,
      pattern: { start: { variable, labels: [type.labels[0]!] }, hops: [] },
      where: and(
        bin("=", prop(v(variable), type.key.property), prop(v("p"), end)),
        where,
      ),
    });
    const text = printClauses([
      { kind: "unwind", expr: pairs, alias: "p" },
      match("m", related, "r", condition),
      ...(selfGuard ? [match("n", node, "s", selfGuard)] : []),
      { kind: "return", items: [{ expr: prop(v("p"), "i"), alias: "i" }] },
    ]);
    const param = Object.keys(ctx.params).find((k) => ctx.params[k] === slot)!;
    return {
      kind: "check",
      statement: { text, params: ctx.params },
      slot: param,
      events: [first],
    };
  }

  /**
   * Inside a deleting transaction, before anything is deleted: which of
   * the doomed nodes each subscriber following one of their keys may
   * read, kept for the change's DELETE events. A node cannot be checked
   * once it is gone, so a deletion nobody checked reaches nobody whose
   * rules depend on the node.
   */
  async probeDeletes(
    change: WriteChange,
    doomed: ReadonlyArray<{ node: NodeType; keys: unknown[] }>,
    run: (statement: Statement) => Promise<QueryResult>,
  ): Promise<void> {
    const wanted = new Map<DeleteProbe, unknown[]>();
    for (const { node, keys } of doomed) {
      const group = this.#subscribers.get(node.name);
      if (!group || group.byKey.size === 0) continue;
      for (const key of keys) {
        for (const sink of group.byKey.get(keyOf(key)) ?? []) {
          const probe = this.#probes.get(sink);
          if (!probe) continue;
          const list = wanted.get(probe);
          if (list) list.push(key);
          else wanted.set(probe, [key]);
        }
      }
    }
    if (wanted.size === 0) return;
    let seen = this.#deleteSeen.get(change);
    if (!seen) {
      seen = new Map();
      this.#deleteSeen.set(change, seen);
    }
    // Subscribers whose checks compile to the same statement share it.
    const results = new Map<string, Promise<Set<string>>>();
    for (const [probe, keys] of wanted) {
      let visible: Promise<Set<string>>;
      if (probe.check.settled()) {
        visible = Promise.resolve(new Set(keys.map(keyOf)));
      } else {
        let statement: Statement;
        try {
          statement = probe.check.statement(keys);
        } catch {
          // The claims alone decide against reading: nothing is visible.
          continue;
        }
        const id = stableKey(statement);
        const shared = id === undefined ? undefined : results.get(id);
        visible =
          shared ??
          run(statement).then(
            (r) => new Set(r.rows.map((row) => keyOf(row["key"]))),
          );
        if (id !== undefined && !shared) results.set(id, visible);
      }
      const own = seen.get(probe) ?? new Set<string>();
      for (const k of await visible) own.add(k);
      seen.set(probe, own);
    }
  }

  /**
   * A changed node for a subscriber. The read is compiled once per
   * subscription (context), field node, claims and variables, with the
   * key as its only varying parameter.
   */
  resolveByKey(
    node: NodeType,
    key: unknown,
    info: GraphQLResolveInfo,
    context: unknown,
    change?: object,
  ): Promise<unknown> {
    const field = info.fieldNodes[0]!;
    const claims = stableKey(this.#host.jwt(context) ?? null);
    const variables = stableKey(coercedVariables(info.variableValues));
    const byField =
      context !== null && typeof context === "object"
        ? (this.#byKeyCompiles.get(context) ??
          this.#byKeyCompiles.set(context, new WeakMap()).get(context)!)
        : undefined;
    let entry = byField?.get(field);
    if (
      !entry ||
      claims === undefined ||
      variables === undefined ||
      entry.claims !== claims ||
      entry.variables !== variables ||
      entry.statistics !== this.#host.statisticsVersion() ||
      !sameContextReads(entry.contextReads, context)
    ) {
      const ctx = this.#host.context(infoContext(info), context);
      const slot = {};
      const compiled = compileRoot(
        ctx,
        "single",
        node,
        { [node.key.name]: slot },
        info.fieldNodes,
      );
      entry =
        compiled.statements.length === 1
          ? slotCompile(
              compiled,
              slot,
              claims,
              this.#host.statisticsVersion(),
              variables,
              ctx.contextReads,
              compiled.statements[0]!,
            )
          : undefined;
      if (!entry) {
        // Not reusable: compile again with the key itself.
        const direct = compileRoot(
          this.#host.context(infoContext(info), context),
          "single",
          node,
          { [node.key.name]: key },
          info.fieldNodes,
        );
        return this.#host.run(info.fieldName, direct, context, info, change);
      }
      if (claims !== undefined && variables !== undefined) {
        byField?.set(field, entry);
      }
    }
    const statement = entry.compiled.statements[0]!;
    const compiled: CompiledRead = {
      ...entry.compiled,
      statements: [
        {
          text: statement.text,
          params: { ...statement.params, [entry.param]: key },
        },
      ],
    };
    const share =
      entry.share === undefined ? undefined : `${entry.share}\0${keyOf(key)}`;
    return this.#host.run(
      info.fieldName,
      compiled,
      context,
      info,
      change,
      share,
    );
  }

  /** `run` once per change for each distinct statement set. */
  shared(
    change: object | undefined,
    statements: Statement[],
    run: () => Promise<QueryResult[]>,
    /** Precomputed key of the statements, from a reused compile. */
    shareKey?: string,
  ): Promise<QueryResult[]> {
    const key = change ? (shareKey ?? stableKey(statements)) : undefined;
    if (!change || key === undefined) return run();
    let reads = this.#sharedReads.get(change);
    if (!reads) {
      reads = new Map();
      this.#sharedReads.set(change, reads);
    }
    let pending = reads.get(key);
    if (!pending) {
      pending = run();
      reads.set(key, pending);
    }
    return pending;
  }
}
