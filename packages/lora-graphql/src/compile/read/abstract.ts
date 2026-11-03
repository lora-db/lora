// Interfaces and unions: root lists, relationship fields whose target is
// abstract, and the per-member projection of an abstract node.

import type { FieldNode, GraphQLObjectType, SelectionSetNode } from "graphql";
import { memberWhere, rootFilterCost } from "../cost.js";
import { requestError } from "../../errors.js";
import type {
  AbstractType,
  NodeType,
  RelationshipField,
  ScalarField,
} from "../../model/types.js";
import {
  authFilter,
  checkAuthentication,
  checkFieldAuthentication,
} from "../auth.js";
import { bind, freshVar, type CompileContext } from "../context.js";
import {
  and,
  bin,
  fn,
  isNull,
  lit,
  or,
  printClauses,
  prop,
  v,
  type Clause,
  type Expr,
  type SortItem,
} from "../cypher.js";
import { decodeCursor } from "../cursor.js";
import type { Statement } from "../../driver.js";
import { compileMemberWhere, relationshipPattern } from "../filter.js";
import { memberFields } from "../../model/relations.js";
import { collectFields, subSelections } from "../selection.js";
import type {
  CompiledRead,
  Args,
  Where,
  Projection,
  RawConnection,
} from "./types.js";
import {
  finish,
  refuseRowRules,
  collectOne,
  coalesceFalse,
  readSet,
  deniedItem,
  assertNoneDenied,
} from "./common.js";
import { resolveLimit } from "./sort.js";
import { projectNode } from "./project.js";

// ---------------------------------------------------------------------------
// Interfaces and unions
// ---------------------------------------------------------------------------

interface MemberSort {
  name: string;
  direction: "ASC" | "DESC";
  /** `@sortable(nulls:)` of the field, as declared on the interface. */
  nulls?: "FIRST" | "LAST" | undefined;
}

function resolveAbstractSort(
  ctx: CompileContext,
  abstract: AbstractType,
  value: unknown,
): MemberSort[] {
  const keys: MemberSort[] = [];
  for (const item of (value as Array<Record<string, unknown>> | null) ?? []) {
    const entries = Object.entries(item).filter(([, d]) => d != null);
    if (entries.length !== 1) {
      throw requestError(
        "BAD_USER_INPUT",
        "each `sort` item names exactly one field",
      );
    }
    const [name, direction] = entries[0]!;
    for (const member of abstract.members) {
      const f = ctx.model.nodes.get(member)!.fields.get(name);
      if (f?.kind === "scalar") {
        checkFieldAuthentication(ctx, member, f);
        refuseRowRules(ctx, ctx.model.nodes.get(member)!, f, "sort by");
      }
    }
    const declared = abstract.fields.get(name);
    keys.push({
      name,
      direction: direction as "ASC" | "DESC",
      ...(declared?.sortNulls && !declared.required
        ? { nulls: declared.sortNulls }
        : {}),
    });
  }
  return keys;
}

/**
 * One member's slice of an interface or union list: its nodes filtered,
 * sorted and limited on their own (so each can use its indexes), projected
 * with `__typename` and the sort values the merge orders by.
 */
function memberList(
  ctx: CompileContext,
  abstract: AbstractType,
  member: NodeType,
  source: (x: string) => {
    pattern: import("../cypher.js").Pattern;
    where: Expr | undefined;
  },
  where: Record<string, unknown> | null | undefined,
  sort: MemberSort[],
  limit: Expr,
  sets: SelectionSetNode[],
  rows: number,
  /** The key's direction: DESC when a connection pages backward. */
  tie: "ASC" | "DESC" = "ASC",
): { body: Clause[]; list: string } | undefined {
  const x = freshVar(ctx, `this_${member.name}`);
  const filter = compileMemberWhere(ctx, abstract, member, x, where);
  if (filter === false) return undefined;
  checkAuthentication(ctx, member, "READ");
  ctx.reads.labels.add(member.labels[0]!);
  const from = source(x);
  const sortExprs: SortItem[] = [
    ...sort.map((k) => {
      const f = member.fields.get(k.name) as ScalarField;
      return {
        expr: prop(v(x), f.property),
        direction: k.direction,
        nulls: k.nulls,
      };
    }),
    { expr: prop(v(x), member.key.property), direction: tie },
  ];
  const projection = projectNode(ctx, member, x, sets, rows);
  const expr = projection.expr as Extract<Expr, { kind: "mapProjection" }>;
  const list = freshVar(ctx, `${x}_list`);
  const tagged: Expr = {
    ...expr,
    entries: [
      ...expr.entries,
      { kind: "entry", key: "__typename", value: lit(member.name) },
      { kind: "entry", key: "__key", value: prop(v(x), member.key.property) },
      ...sort.map((k, i) => ({
        kind: "entry" as const,
        key: `__s${i}`,
        value: sortExprs[i]!.expr,
      })),
    ],
  };
  return {
    list,
    body: [
      {
        kind: "match",
        pattern: from.pattern,
        where: and(from.where, filter, authFilter(ctx, member, x, "READ")),
      },
      { kind: "with", items: [{ expr: v(x) }], orderBy: sortExprs, limit },
      ...projection.pre,
      { kind: "return", items: [{ expr: fn("collect", tagged), alias: list }] },
    ],
  };
}

/** The merged order: requested fields, then type name, then key. */
function mergedOrder(
  item: string,
  sort: MemberSort[],
  tie: "ASC" | "DESC" = "ASC",
): SortItem[] {
  return [
    ...sort.map((k, i) => ({
      expr: prop(v(item), `__s${i}`),
      direction: k.direction,
      nulls: k.nulls,
    })),
    { expr: prop(v(item), "__typename"), direction: tie },
    { expr: prop(v(item), "__key"), direction: tie },
  ];
}

/** `events(where, sort, limit)` over an interface's implementations or a union's members. */
export function compileAbstractRoot(
  ctx: CompileContext,
  abstract: AbstractType,
  args: Args,
  fieldNodes: readonly FieldNode[],
): CompiledRead {
  const limitValue = resolveLimit(args["limit"], abstract.limit, "limit");
  const sort = resolveAbstractSort(ctx, abstract, args["sort"]);
  const limit = bind(ctx, limitValue);
  const sets = subSelections(fieldNodes);
  const calls: Clause[] = [];
  const lists: string[] = [];
  for (const name of abstract.members) {
    const member = ctx.model.nodes.get(name)!;
    const slice = memberList(
      ctx,
      abstract,
      member,
      (x) => ({
        pattern: {
          start: { variable: x, labels: [member.labels[0]!] },
          hops: [],
        },
        where: undefined,
      }),
      args["where"] as Record<string, unknown> | undefined,
      sort,
      limit,
      sets,
      limitValue,
    );
    if (!slice) continue;
    ctx.cost += rootFilterCost(
      ctx,
      member,
      memberWhere(abstract, member.name, args["where"] as Where | undefined),
      limitValue,
    );
    calls.push({ kind: "call", imports: [], body: slice.body });
    lists.push(slice.list);
  }
  const clauses: Clause[] = [
    ...calls,
    {
      kind: "unwind",
      expr:
        lists.length === 0
          ? { kind: "list", items: [] }
          : lists.map((l) => v(l)).reduce((a, b) => bin("+", a, b)),
      alias: "this",
    },
    {
      kind: "with",
      items: [{ expr: v("this") }],
      orderBy: mergedOrder("this", sort),
      limit,
    },
    { kind: "return", items: [{ expr: v("this"), alias: "this" }] },
  ];
  return finish(
    ctx,
    clauses,
    ["this"],
    ([r]) => r!.rows.map((row) => row["this"]),
    undefined,
  );
}

/**
 * `eventsConnection(first, after, last, before, where, sort)` over an
 * interface's implementations or a union's members: each member's page
 * (filtered, keyset-bounded, sorted and limited on its own, so each can
 * use its indexes) merged in the list root's order (requested fields,
 * then type name, then key), and `totalCount` summed across members.
 * A cursor holds the sort values, the type name and the key.
 */
export function compileAbstractConnection(
  ctx: CompileContext,
  abstract: AbstractType,
  args: Args,
  fieldNodes: readonly FieldNode[],
): CompiledRead {
  const backward = args["last"] != null || args["before"] != null;
  if (backward && (args["first"] != null || args["after"] != null)) {
    throw requestError(
      "BAD_USER_INPUT",
      "page forward with first/after or backward with last/before, not both",
    );
  }
  const first = resolveLimit(
    backward ? args["last"] : args["first"],
    abstract.limit,
    backward ? "last" : "first",
  );
  const requested = resolveAbstractSort(ctx, abstract, args["sort"]);
  const signature = [
    `${abstract.name}:`,
    ...requested.map(
      (k) => `${k.name}:${k.direction}${k.nulls ? `:NULLS_${k.nulls}` : ""}`,
    ),
  ].join(",");
  const raw = (backward ? args["before"] : args["after"]) as
    | string
    | null
    | undefined;
  const cursor =
    raw != null
      ? decodeCursor(
          raw,
          signature,
          requested.length + 2,
          ctx.model.cursorSecret,
        )
      : undefined;
  const flip = (d: "ASC" | "DESC") => (d === "ASC" ? "DESC" : "ASC");
  const sort: MemberSort[] = backward
    ? requested.map((k) => ({
        name: k.name,
        direction: flip(k.direction),
        ...(k.nulls ? { nulls: k.nulls === "LAST" ? "FIRST" : "LAST" } : {}),
      }))
    : requested;
  const tie = backward ? "DESC" : "ASC";
  const where = args["where"] as Record<string, unknown> | undefined;

  const set = subSelections(fieldNodes);
  const connType = ctx.schema.getType(
    `${abstract.name}Connection`,
  ) as GraphQLObjectType;
  const edgeType = ctx.schema.getType(
    `${abstract.name}Edge`,
  ) as GraphQLObjectType;
  // What the selection asks for, by field name (a response key may be an
  // alias): the page for `edges` or `pageInfo`, the count for `totalCount`.
  let page = false;
  let counted = false;
  const nodeSets: SelectionSetNode[] = [];
  for (const [, nodes] of collectFields(ctx, connType, set)) {
    const name = nodes[0]!.name.value;
    if (name === "totalCount") counted = true;
    if (name === "pageInfo") page = true;
    if (name !== "edges") continue;
    page = true;
    for (const [, edgeNodes] of collectFields(
      ctx,
      edgeType,
      subSelections(nodes),
    )) {
      if (edgeNodes[0]!.name.value === "node") {
        nodeSets.push(...subSelections(edgeNodes));
      }
    }
  }

  const statements: Statement[] = [];
  const members = abstract.members.map((m) => ctx.model.nodes.get(m)!);
  if (page) {
    const limit = bind(ctx, first + 1);
    const calls: Clause[] = [];
    const lists: string[] = [];
    for (const member of members) {
      const slice = memberList(
        ctx,
        abstract,
        member,
        (x) => ({
          pattern: {
            start: { variable: x, labels: [member.labels[0]!] },
            hops: [],
          },
          where: cursor
            ? memberKeyset(ctx, member, x, sort, tie, cursor)
            : undefined,
        }),
        where,
        sort,
        limit,
        nodeSets,
        first + 1,
        tie,
      );
      if (!slice) continue;
      ctx.cost += rootFilterCost(
        ctx,
        member,
        memberWhere(abstract, member.name, where as Where | undefined),
        first + 1,
      );
      calls.push({ kind: "call", imports: [], body: slice.body });
      lists.push(slice.list);
    }
    statements.push({
      text: printClauses([
        ...calls,
        {
          kind: "unwind",
          expr:
            lists.length === 0
              ? { kind: "list", items: [] }
              : lists.map((l) => v(l)).reduce((a, b) => bin("+", a, b)),
          alias: "this",
        },
        {
          kind: "with",
          items: [{ expr: v("this") }],
          orderBy: mergedOrder("this", sort, tie),
          limit,
        },
        {
          kind: "return",
          items: [
            { expr: v("this"), alias: "node" },
            {
              expr: {
                kind: "list",
                items: [
                  ...sort.map((_, i) => prop(v("this"), `__s${i}`)),
                  prop(v("this"), "__typename"),
                  prop(v("this"), "__key"),
                ],
              },
              alias: "__cursor",
            },
          ],
        },
      ]),
      params: ctx.params,
    });
  }

  // totalCount: every member's matches, in the same read transaction.
  const capped = ctx.maxTotalCount;
  let countCost = 0;
  if (counted) {
    const statsCtx: CompileContext = {
      ...ctx,
      params: {},
      vars: new Set(["this"]),
      computed: new Map(),
    };
    const calls: Clause[] = [];
    const counts: string[] = [];
    const denied: string[] = [];
    for (const member of members) {
      const x = freshVar(statsCtx, `this_${member.name}`);
      const filter = compileMemberWhere(statsCtx, abstract, member, x, where);
      if (filter === false) continue;
      checkAuthentication(statsCtx, member, "READ");
      ctx.reads.labels.add(member.labels[0]!);
      const count = freshVar(statsCtx, `${x}_count`);
      const items: Array<{ expr: Expr; alias: string }> = [
        { expr: fn("count", v(x)), alias: count },
        ...deniedItem(statsCtx, member, x).map((item) => ({
          ...item,
          alias: freshVar(statsCtx, `${x}_denied`),
        })),
      ];
      if (items.length > 1) denied.push(items[1]!.alias);
      calls.push({
        kind: "call",
        imports: [],
        body: [
          {
            kind: "match",
            pattern: {
              start: { variable: x, labels: [member.labels[0]!] },
              hops: [],
            },
            where: and(filter, authFilter(statsCtx, member, x, "READ")),
          },
          ...(capped === undefined
            ? []
            : [
                {
                  kind: "with" as const,
                  items: [{ expr: v(x) }],
                  limit: bind(statsCtx, capped),
                },
              ]),
          { kind: "return", items },
        ],
      });
      counts.push(count);
      const memberFilter = memberWhere(
        abstract,
        member.name,
        where as Where | undefined,
      );
      const all = rootFilterCost(ctx, member, memberFilter, first + 1, true);
      countCost +=
        capped !== undefined && !memberFilter ? Math.min(capped, all) : all;
    }
    const sum = (vars: string[]): Expr =>
      vars.length === 0
        ? lit(0)
        : vars.map((c) => v(c)).reduce((a, b) => bin("+", a, b));
    const total = sum(counts);
    statements.push({
      text: printClauses([
        ...calls,
        {
          kind: "return",
          items: [
            {
              expr:
                capped === undefined || counts.length < 2
                  ? total
                  : {
                      kind: "case",
                      when: bin(">", total, bind(statsCtx, capped)),
                      then: bind(statsCtx, capped),
                      else: total,
                    },
              alias: "totalCount",
            },
            ...(denied.length > 0
              ? [{ expr: sum(denied), alias: "__denied" }]
              : []),
          ],
        },
      ]),
      params: statsCtx.params,
    });
  }

  return {
    statements,
    columns: page ? ["node", "__cursor"] : counted ? ["totalCount"] : [],
    reads: readSet(ctx),
    expectations: [],
    cost: ctx.cost + (counted ? Math.max(1, countCost) : 0),
    mode: "read",
    shape: (results): RawConnection => {
      const rows = page ? results[0] : undefined;
      const count = counted ? results[page ? 1 : 0] : undefined;
      const row = count?.rows[0];
      if (count) assertNoneDenied(row);
      return {
        __rows: (rows?.rows ?? []).map((r) => ({
          node: r["node"],
          __cursor: r["__cursor"] as unknown[],
        })),
        __first: first,
        __after: cursor !== undefined,
        __backward: backward,
        __sort: signature,
        ...(count ? { __totalCount: Number(row?.["totalCount"] ?? 0) } : {}),
      };
    },
  };
}

/**
 * A member's rows strictly after `cursor` (sort values, type name, key)
 * in the merged order. The type name is the member's own, so its term is
 * decided here: a member after the cursor's type takes every row tied on
 * the sort values, the cursor's own type the rows past its key.
 */
function memberKeyset(
  ctx: CompileContext,
  member: NodeType,
  x: string,
  sort: MemberSort[],
  tie: "ASC" | "DESC",
  cursor: unknown[],
): Expr {
  const branches: Expr[] = [];
  const equal: Expr[] = [];
  sort.forEach((k, i) => {
    const f = member.fields.get(k.name) as ScalarField;
    const target = prop(v(x), f.property);
    const value = cursor[i];
    const last = k.nulls ? k.nulls === "LAST" : k.direction === "ASC";
    let after: Expr | undefined;
    if (value === null || value === undefined) {
      after = last ? undefined : isNull(target, true);
    } else {
      const p = bind(ctx, value);
      after = bin(k.direction === "ASC" ? ">" : "<", target, p);
      if (last) after = or(after, isNull(target));
    }
    if (after) branches.push(and(...equal, after)!);
    equal.push(
      value === null || value === undefined
        ? isNull(target)
        : bin("=", target, bind(ctx, value)),
    );
  });
  const typename = cursor[sort.length] as string;
  const later = tie === "ASC" ? member.name > typename : member.name < typename;
  if (later) {
    branches.push(and(...equal) ?? lit(true));
  } else if (member.name === typename) {
    branches.push(
      and(
        ...equal,
        bin(
          tie === "ASC" ? ">" : "<",
          prop(v(x), member.key.property),
          bind(ctx, cursor[sort.length + 1]),
        ),
      )!,
    );
  }
  const disjunction = or(...branches) ?? lit(false);
  // A non-null lead value also bounds the lead, so a range index can seek
  // it; with its nulls last they are a disjunct of their own.
  const lead = sort[0];
  const value = cursor[0];
  if (!lead || value === null || value === undefined) return disjunction;
  const f = member.fields.get(lead.name) as ScalarField;
  const target = prop(v(x), f.property);
  const last = lead.nulls ? lead.nulls === "LAST" : lead.direction === "ASC";
  return or(
    and(
      bin(lead.direction === "ASC" ? ">=" : "<=", target, bind(ctx, value)),
      disjunction,
    ),
    last ? isNull(target) : undefined,
  )!;
}

/** A relationship field whose target is an interface or union. */
export function projectPolymorphic(
  ctx: CompileContext,
  parent: string,
  rel: RelationshipField,
  key: string,
  args: Args,
  sets: SelectionSetNode[],
  rows: number,
): Projection {
  const abstract = ctx.model.abstracts.get(rel.target)!;
  ctx.reads.relationships.add(rel.type);
  const limitValue = rel.list
    ? resolveLimit(args["limit"], rel.limit ?? abstract.limit, "limit")
    : 1;
  const sort = rel.list ? resolveAbstractSort(ctx, abstract, args["sort"]) : [];
  const limit = bind(ctx, limitValue);
  const body: Clause[] = [];
  const lists: string[] = [];
  for (const field of memberFields(ctx.model, rel)) {
    const member = ctx.model.nodes.get(field.target)!;
    const slice = memberList(
      ctx,
      abstract,
      member,
      (x) => ({
        pattern: relationshipPattern(parent, field, member.labels[0]!, x),
        where: undefined,
      }),
      args["where"] as Record<string, unknown> | undefined,
      sort,
      limit,
      sets,
      rows * limitValue,
    );
    if (!slice) continue;
    body.push({ kind: "call", imports: [parent], body: slice.body });
    lists.push(slice.list);
  }
  if (lists.length === 0) {
    return {
      pre: [],
      expr: rel.list ? { kind: "list", items: [] } : lit(null),
    };
  }
  const item = freshVar(ctx, `${parent}_${key}_item`);
  const out = freshVar(ctx, `${parent}_${key}`);
  body.push(
    {
      kind: "unwind",
      expr: lists.map((l) => v(l)).reduce((a, b) => bin("+", a, b)),
      alias: item,
    },
    {
      kind: "with",
      items: [{ expr: v(item) }],
      orderBy: mergedOrder(item, sort),
      limit,
    },
    ...(rel.list
      ? [
          {
            kind: "return" as const,
            items: [{ expr: fn("collect", v(item)), alias: out }],
          },
        ]
      : collectOne(v(item), out)),
  );
  return { pre: [{ kind: "call", imports: [parent], body }], expr: v(out) };
}

/**
 * A node `variable` of an interface or union, projected as the member its
 * label says, with `__typename`. `where` keeps the nodes of some member
 * the caller may read; others (and non-members) are dropped.
 */
export function projectAbstract(
  ctx: CompileContext,
  abstract: AbstractType,
  variable: string,
  sets: SelectionSetNode[],
  rows: number,
): { pre: Clause[]; where: Expr; expr: Expr } {
  const pre: Clause[] = [];
  const branches: Array<{ when: Expr; then: Expr }> = [];
  for (const name of abstract.members) {
    const member = ctx.model.nodes.get(name)!;
    checkAuthentication(ctx, member, "READ");
    ctx.reads.labels.add(member.labels[0]!);
    const projection = projectNode(ctx, member, variable, sets, rows);
    pre.push(...projection.pre);
    const expr = projection.expr as Extract<Expr, { kind: "mapProjection" }>;
    branches.push({
      when:
        and(
          { kind: "hasLabels", variable, labels: [member.labels[0]!] },
          coalesceFalse(authFilter(ctx, member, variable, "READ")),
        ) ?? lit(true),
      then: {
        ...expr,
        entries: [
          ...expr.entries,
          { kind: "entry", key: "__typename", value: lit(member.name) },
        ],
      },
    });
  }
  const chain = branches.reduceRight<Expr>(
    (otherwise, b) => ({
      kind: "case",
      when: b.when,
      then: b.then,
      else: otherwise,
    }),
    lit(null),
  );
  return {
    pre,
    where: or(...branches.map((b) => b.when)) ?? lit(false),
    expr: chain,
  };
}
