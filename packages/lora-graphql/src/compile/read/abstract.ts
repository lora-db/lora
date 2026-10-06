// Interfaces and unions: root lists, relationship fields whose target is
// abstract, and the per-member projection of an abstract node.

import type { FieldNode, SelectionSetNode } from "graphql";
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
  lit,
  or,
  prop,
  v,
  type Clause,
  type Expr,
  type SortItem,
} from "../cypher.js";
import { compileMemberWhere, relationshipPattern } from "../filter.js";
import { memberFields } from "../../model/relations.js";
import { subSelections } from "../selection.js";
import type { CompiledRead, Args, Where, Projection } from "./types.js";
import { finish, refuseRowRules, collectOne, coalesceFalse } from "./common.js";
import { resolveLimit } from "./sort.js";
import { projectNode } from "./project.js";

// ---------------------------------------------------------------------------
// Interfaces and unions
// ---------------------------------------------------------------------------

interface MemberSort {
  name: string;
  direction: "ASC" | "DESC";
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
    keys.push({ name, direction: direction as "ASC" | "DESC" });
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
      return { expr: prop(v(x), f.property), direction: k.direction };
    }),
    { expr: prop(v(x), member.key.property), direction: "ASC" as const },
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
function mergedOrder(item: string, sort: MemberSort[]): SortItem[] {
  return [
    ...sort.map((k, i) => ({
      expr: prop(v(item), `__s${i}`),
      direction: k.direction,
    })),
    { expr: prop(v(item), "__typename"), direction: "ASC" as const },
    { expr: prop(v(item), "__key"), direction: "ASC" as const },
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
