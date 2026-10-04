// `where` inputs → predicates. Specialised to the input actually supplied:
// an absent or null operator adds nothing to the statement (rule 7), so
// each variant keeps its predicates visible to the planner.

import { requestError } from "../errors.js";
import { toLoraPoint } from "../model/points.js";
import { abstractOf, memberFields } from "../model/relations.js";
import type {
  AbstractType,
  NodeType,
  RelationshipField,
  RelationshipPropertiesType,
  ScalarField,
} from "../model/types.js";
import {
  distinctCount,
  listAggregate,
  type ListAggregate,
} from "./aggregate.js";
import {
  authFilter,
  checkAuthentication,
  checkFieldAuthentication,
  checkPropertyAccess,
  fieldValidate,
  maskedValue,
  maskSettled,
  refuseEdgeRowRules,
} from "./auth.js";
import { bind, freshVar, type CompileContext } from "./context.js";
import {
  and,
  bin,
  fn,
  isNull,
  lit,
  not,
  or,
  prop,
  v,
  type BinaryOp,
  type Expr,
  type Pattern,
} from "./cypher.js";

type Where = Record<string, unknown>;

const SCALAR_OPS: Record<string, BinaryOp> = {
  eq: "=",
  in: "IN",
  lt: "<",
  lte: "<=",
  gt: ">",
  gte: ">=",
  contains: "CONTAINS",
  startsWith: "STARTS WITH",
  endsWith: "ENDS WITH",
};

const COUNT_OPS: Record<string, BinaryOp> = {
  eq: "=",
  lt: "<",
  lte: "<=",
  gt: ">",
  gte: ">=",
};

/** Compiles one `where` key's value; undefined when it adds nothing. */
type KeyCompiler = (value: unknown) => Expr | undefined;

export function compileNodeWhere(
  ctx: CompileContext,
  node: NodeType,
  variable: string,
  where: Where | null | undefined,
): Expr | undefined {
  return compileWhere(ctx, where, nodeKeys(ctx, node, variable));
}

function nodeKeys(
  ctx: CompileContext,
  node: NodeType,
  variable: string,
): (key: string) => KeyCompiler | undefined {
  return (key) => {
    const f = node.fields.get(key);
    if (f?.kind === "scalar") {
      // Filtering on a field reveals it: the same rules as reading it.
      checkFieldAuthentication(ctx, node.name, f);
      const rule = fieldValidate(ctx, node, f, variable, "READ");
      return (value) => {
        // A masked field is compared as the reader sees it.
        const pred = scalarPredicate(
          ctx,
          maskedValue(ctx, node, f, variable, prop(v(variable), f.property)),
          value as Where,
        );
        return pred && guarded(rule, pred);
      };
    }
    if (f?.kind === "relationship") {
      checkFieldAuthentication(ctx, node.name, f);
      // Filtering through the field reveals it, as reading it does.
      const rule = fieldValidate(ctx, node, f, variable, "READ");
      return (value) => {
        const pred = relationshipPredicate(ctx, variable, f, value as Where);
        return pred && guarded(rule, pred);
      };
    }
    if (f?.kind === "cypher" && f.computed) {
      checkFieldAuthentication(ctx, node.name, f);
      const bound = ctx.computed.get(`${variable}\0${f.name}`);
      if (!bound) {
        throw requestError(
          "BAD_USER_INPUT",
          `${node.name}.${f.name} is computed by @cypher: filter by it in a root field's where, not through a relationship or search`,
        );
      }
      const rule = fieldValidate(ctx, node, f, variable, "READ");
      return (value) => {
        const pred = scalarPredicate(ctx, v(bound), value as Where);
        return pred && guarded(rule, pred);
      };
    }
    // `<field>Exists`: whether a single relationship is set. A related node
    // the reader may not see counts as none, as in every relationship
    // filter (a rule sees every node).
    if (key.endsWith("Exists")) {
      const rel = node.fields.get(key.slice(0, -"Exists".length));
      if (rel?.kind === "relationship" && !rel.list) {
        checkFieldAuthentication(ctx, node.name, rel);
        const rule = fieldValidate(ctx, node, rel, variable, "READ");
        return (value) => {
          if (typeof value !== "boolean") return undefined;
          const pred = relationshipExists(ctx, variable, rel, value);
          return guarded(rule, pred);
        };
      }
    }
    // `<field>Connection`: quantifiers over { node, edge } of a list
    // relationship with properties.
    if (key.endsWith("Connection")) {
      const rel = node.fields.get(key.slice(0, -"Connection".length));
      if (rel?.kind === "relationship" && rel.list && rel.properties) {
        checkFieldAuthentication(ctx, node.name, rel);
        const rule = fieldValidate(ctx, node, rel, variable, "READ");
        return (value) => {
          const pred = connectionPredicate(ctx, variable, rel, value as Where);
          return pred && guarded(rule, pred);
        };
      }
    }
    return undefined;
  };
}

/**
 * A filter on a field with field-level READ rules: it matches only rows
 * passing the rule. The rule is read as false where it is unknown (a null
 * property), so the guarded predicate is never NULL on a row failing it:
 * `NOT` over it is then true on every such row, whatever the hidden value,
 * instead of true exactly where the value fails the predicate. Under no
 * `NOT` the coalesce changes nothing (NULL and false both drop the row).
 */
function guarded(rule: Expr | undefined, pred: Expr): Expr {
  return rule ? and(fn("coalesce", rule, lit(false)), pred)! : pred;
}

export function compilePropsWhere(
  ctx: CompileContext,
  props: RelationshipPropertiesType,
  variable: string,
  where: Where | null | undefined,
): Expr | undefined {
  return compileWhere(ctx, where, (key) => {
    const f = props.fields.get(key);
    // Filtering on a property reveals it: the same rules as reading it.
    if (f) checkPropertyAccess(ctx, props.name, f, "READ");
    return f
      ? (value) =>
          scalarPredicate(ctx, prop(v(variable), f.property), value as Where)
      : undefined;
  });
}

function compileWhere(
  ctx: CompileContext,
  where: Where | null | undefined,
  lookup: (key: string) => KeyCompiler | undefined,
): Expr | undefined {
  if (!where) return undefined;
  const parts: Array<Expr | undefined> = [];
  for (const [key, value] of Object.entries(where)) {
    if (value === null || value === undefined) continue;
    if (key === "AND") {
      for (const w of value as Where[]) {
        parts.push(compileWhere(ctx, w, lookup));
      }
    } else if (key === "OR") {
      const list = value as Where[];
      // A literal `OR: []` matches nothing. A branch left empty once its
      // absent and null filters are left out is itself left out, and an
      // OR with no branch left adds nothing: absent variables never widen
      // a filter to every row.
      if (list.length === 0) {
        parts.push(lit(false));
        continue;
      }
      const branches = list
        .map((w) => compileWhere(ctx, w, lookup))
        .filter((b): b is Expr => b !== undefined);
      if (branches.length > 0) parts.push(or(...branches));
    } else if (key === "NOT") {
      // A NOT with nothing left inside adds nothing (not "NOT TRUE"). In
      // an authorization rule it still denies: a rule never widens.
      const inner = compileWhere(ctx, value as Where, lookup);
      if (inner) parts.push(not(inner));
      else if (ctx.inAuth) parts.push(lit(false));
    } else {
      const compile = lookup(key);
      // In a rule, a key that compiles to nothing would silently widen
      // it: deny instead. The model checks refuse such keys at startup.
      if (!compile && ctx.inAuth) parts.push(lit(false));
      else parts.push(compile?.(value));
    }
  }
  return and(...parts);
}

function scalarPredicate(
  ctx: CompileContext,
  target: Expr,
  ops: Where,
): Expr | undefined {
  const parts: Expr[] = [];
  for (const [op, value] of Object.entries(ops)) {
    if (value === null || value === undefined) continue;
    switch (op) {
      case "withinBBox": {
        const box = value as { lowerLeft: unknown; upperRight: unknown };
        parts.push(
          fn(
            "geo.within_bbox",
            target,
            bind(ctx, toLoraPoint(box.lowerLeft)),
            bind(ctx, toLoraPoint(box.upperRight)),
          ),
        );
        continue;
      }
      case "distance": {
        const d = value as { from: unknown; lte: number };
        parts.push(
          bin(
            "<=",
            fn("geo.distance", target, bind(ctx, toLoraPoint(d.from))),
            bind(ctx, d.lte),
          ),
        );
        continue;
      }
      case "includes":
        parts.push(bin("IN", bind(ctx, value), target));
        continue;
      case "isNull":
        parts.push(isNull(target, value === false));
        continue;
      case "caseInsensitive": {
        const lowered = fn("toLower", target);
        for (const [ciOp, ciValue] of Object.entries(value as Where)) {
          if (ciValue === null || ciValue === undefined) continue;
          const cypherOp = SCALAR_OPS[ciOp];
          if (!cypherOp) continue;
          const lower = Array.isArray(ciValue)
            ? ciValue.map((s) => String(s).toLowerCase())
            : String(ciValue).toLowerCase();
          parts.push(bin(cypherOp, lowered, bind(ctx, lower)));
        }
        continue;
      }
    }
    const cypherOp = SCALAR_OPS[op];
    if (!cypherOp) continue;
    parts.push(bin(cypherOp, target, bind(ctx, value)));
  }
  return and(...parts);
}

/**
 * `(from)-[:TYPE]->(to:Target)` for a relationship field. Reads follow an
 * UNDIRECTED relationship both ways.
 */
export function relationshipPattern(
  from: string,
  rel: RelationshipField,
  targetLabel: string,
  to: string,
  relVariable?: string,
): Pattern {
  return {
    start: { variable: from, labels: [] },
    hops: [
      {
        rel: {
          ...(relVariable !== undefined ? { variable: relVariable } : {}),
          type: rel.type,
          direction:
            rel.queryDirection === "UNDIRECTED" ? "BOTH" : rel.direction,
        },
        node: { variable: to, labels: [targetLabel] },
      },
    ],
  };
}

/**
 * Related nodes as a pattern comprehension:
 * `[(from)-[r:T]->(x:Target) WHERE <visible> AND <where> | <projection>]`.
 */
function related(
  ctx: CompileContext,
  from: string,
  rel: RelationshipField,
  where: (x: string, r: string) => Expr | undefined,
  projection: (x: string, r: string) => Expr,
): Expr {
  const target = ctx.model.nodes.get(rel.target)!;
  const x = freshVar(ctx, `${from}_${rel.name}`);
  const r = freshVar(ctx, `${x}_rel`);
  return {
    kind: "comprehension",
    pattern: relationshipPattern(from, rel, target.labels[0]!, x, r),
    // Related nodes the reader may not see do not count, in any quantifier.
    where: and(authFilter(ctx, target, x, "READ"), where(x, r)),
    projection: projection(x, r),
  };
}

/**
 * The `where` of an interface or union, for one member type: an
 * interface's filters apply to every implementation (and `typename` picks
 * implementations); a union's where names members and leaves out the
 * ones it does not name. False when `where` excludes the member.
 */
export function compileMemberWhere(
  ctx: CompileContext,
  abstract: AbstractType | undefined,
  member: NodeType,
  variable: string,
  where: Where | null | undefined,
): Expr | undefined | false {
  if (!abstract) return compileNodeWhere(ctx, member, variable, where);
  if (!where) return undefined;
  if (abstract.kind === "union") {
    const named = Object.entries(where).filter(([, w]) => w != null);
    if (named.length === 0) return undefined;
    const mine = where[member.name] as Where | null | undefined;
    if (mine == null) return false;
    return compileNodeWhere(ctx, member, variable, mine);
  }
  const { typename, ...rest } = where;
  if (Array.isArray(typename) && !typename.includes(member.name)) return false;
  // `typename` may also sit inside AND / OR / NOT: there it is a constant
  // per member, true or false (not absent: NOT must flip it).
  const keys = nodeKeys(ctx, member, variable);
  return compileWhere(ctx, rest, (key) =>
    key === "typename"
      ? (value) =>
          Array.isArray(value) ? lit(value.includes(member.name)) : undefined
      : keys(key),
  );
}

function relationshipPredicate(
  ctx: CompileContext,
  variable: string,
  rel: RelationshipField,
  value: Where,
): Expr | undefined {
  const abstract = abstractOf(ctx.model, rel);
  const members = memberFields(ctx.model, rel).map((field) => {
    const node = ctx.model.nodes.get(field.target)!;
    checkAuthentication(ctx, node, "READ");
    ctx.reads.labels.add(node.labels[0]!);
    return { field, node };
  });
  ctx.reads.relationships.add(rel.type);
  const memberWhere = (node: NodeType, x: string, where: Where | undefined) =>
    compileMemberWhere(ctx, abstract, node, x, where);
  const sum = (parts: Expr[]): Expr =>
    parts.length === 0 ? lit(0) : parts.reduce((a, b) => bin("+", a, b));

  // size([(this)-[:T]->(x:Target) WHERE pred | 1]): LoraDB has no
  // EXISTS { } subquery, and a pattern comprehension beats OPTIONAL MATCH.
  // Over an interface or union, the members' counts add up.
  const count = (where: Where | undefined) =>
    sum(
      members.flatMap(({ field, node }) => {
        const probe = memberWhere(node, "probe", where);
        if (probe === false) return [];
        return [
          fn(
            "size",
            related(
              ctx,
              variable,
              field,
              (x) => memberWhere(node, x, where) || undefined,
              () => lit(1),
            ),
          ),
        ];
      }),
    );
  // Exact counts count related nodes, once each, however many
  // relationships lead to them.
  const distinct = (where: Where | undefined) => {
    const lists = members.flatMap(({ field, node }) => {
      if (memberWhere(node, "probe", where) === false) return [];
      return [
        related(
          ctx,
          variable,
          field,
          (x) => memberWhere(node, x, where) || undefined,
          (x) => ({
            kind: "list",
            items: [lit(node.name), prop(v(x), node.key.property)],
          }),
        ),
      ];
    });
    if (lists.length === 0) return lit(0);
    return distinctCount(
      ctx,
      lists.reduce((a, b) => bin("+", a, b)),
    );
  };
  // A quantifier over an empty (or all-null) filter is left out, like any
  // absent filter; `count: { gt: 0 }` asks for "has any".
  const nonEmpty = (where: Where): boolean =>
    members.some(({ node }) => {
      const probe = { ...ctx, params: {}, vars: new Set(ctx.vars) };
      const w = compileMemberWhere(probe, abstract, node, "probe", where);
      return w !== undefined;
    });

  if (!rel.list) {
    // A single relationship filters by its target directly.
    return nonEmpty(value) ? bin(">", count(value), lit(0)) : undefined;
  }

  const parts: Array<Expr | undefined> = [];
  for (const [quantifier, inner] of Object.entries(value)) {
    if (inner === null || inner === undefined) continue;
    const w = inner as Where;
    switch (quantifier) {
      case "some":
        if (nonEmpty(w)) parts.push(bin(">", count(w), lit(0)));
        break;
      case "none":
        if (nonEmpty(w)) parts.push(bin("=", count(w), lit(0)));
        break;
      case "single":
        if (nonEmpty(w)) parts.push(bin("=", distinct(w), lit(1)));
        break;
      case "all": {
        // Every related node matches: none fails. Unknown (null) counts
        // as failing, so `all` never holds on missing properties. An
        // empty set satisfies `all`, as in logic and Cypher's all(). A
        // member a union's where leaves out fails.
        if (!nonEmpty(w)) break;
        const failing = sum(
          members.map(({ field, node }) =>
            fn(
              "size",
              related(
                ctx,
                variable,
                field,
                (x) => {
                  const pred = memberWhere(node, x, w);
                  if (pred === false) return undefined;
                  return pred
                    ? not(fn("coalesce", pred, lit(false)))
                    : lit(false);
                },
                () => lit(1),
              ),
            ),
          ),
        );
        parts.push(bin("=", failing, lit(0)));
        break;
      }
      case "count":
        parts.push(compareCount(ctx, rel, distinct(undefined), w));
        break;
      case "aggregate":
        if (!abstract) parts.push(aggregatePredicate(ctx, variable, rel, w));
        break;
    }
  }
  return and(...parts);
}

/** `size([(this)-[:T]->(x:Target) | 1]) > 0` (or `= 0`), over every member. */
function relationshipExists(
  ctx: CompileContext,
  variable: string,
  rel: RelationshipField,
  exists: boolean,
): Expr {
  const counts = memberFields(ctx.model, rel).map((field) => {
    const node = ctx.model.nodes.get(field.target)!;
    checkAuthentication(ctx, node, "READ");
    ctx.reads.labels.add(node.labels[0]!);
    return fn(
      "size",
      related(
        ctx,
        variable,
        field,
        () => undefined,
        () => lit(1),
      ),
    );
  });
  ctx.reads.relationships.add(rel.type);
  const total =
    counts.length === 0 ? lit(0) : counts.reduce((a, b) => bin("+", a, b));
  return bin(exists ? ">" : "=", total, lit(0));
}

function compareCount(
  ctx: CompileContext,
  rel: RelationshipField,
  total: Expr,
  ops: Where,
): Expr | undefined {
  const parts: Expr[] = [];
  for (const [op, n] of Object.entries(ops)) {
    if (n === null || n === undefined) continue;
    const cypherOp = COUNT_OPS[op];
    if (!cypherOp) continue;
    if (!Number.isInteger(n) || (n as number) < 0) {
      throw requestError(
        "BAD_USER_INPUT",
        `${rel.name}.count.${op} must be a non-negative integer`,
      );
    }
    parts.push(bin(cypherOp, total, bind(ctx, n)));
  }
  return and(...parts);
}

/**
 * `followersConnection: { some: { node: {...}, edge: {...} } }`: the
 * quantifiers, over pairs of related node and relationship.
 */
function connectionPredicate(
  ctx: CompileContext,
  variable: string,
  rel: RelationshipField,
  value: Where,
): Expr | undefined {
  const target = ctx.model.nodes.get(rel.target)!;
  const props = ctx.model.relationshipProperties.get(rel.properties!)!;
  checkAuthentication(ctx, target, "READ");
  ctx.reads.labels.add(target.labels[0]!);
  ctx.reads.relationships.add(rel.type);
  const pairWhere = (w: Where) => (x: string, r: string) =>
    and(
      compileNodeWhere(ctx, target, x, w["node"] as Where | undefined),
      compilePropsWhere(ctx, props, r, w["edge"] as Where | undefined),
    );
  const count = (w: Where, negate: boolean) =>
    fn(
      "size",
      related(
        ctx,
        variable,
        rel,
        (x, r) => {
          const p = pairWhere(w)(x, r)!;
          return negate ? not(fn("coalesce", p, lit(false))) : p;
        },
        () => lit(1),
      ),
    );
  const parts: Expr[] = [];
  for (const [quantifier, inner] of Object.entries(value)) {
    if (inner === null || inner === undefined) continue;
    const w = inner as Where;
    if (!nonEmptyPair(ctx, target, props, w)) continue;
    if (w["edge"] != null) refuseEdgeRowRules(ctx, rel);
    switch (quantifier) {
      case "some":
        parts.push(bin(">", count(w, false), lit(0)));
        break;
      case "none":
        parts.push(bin("=", count(w, false), lit(0)));
        break;
      case "single":
        parts.push(bin("=", count(w, false), lit(1)));
        break;
      case "all":
        parts.push(bin("=", count(w, true), lit(0)));
        break;
    }
  }
  return and(...parts);
}

function nonEmptyPair(
  ctx: CompileContext,
  target: NodeType,
  props: RelationshipPropertiesType,
  w: Where,
): boolean {
  const probe = { ...ctx, params: {}, vars: new Set(ctx.vars) };
  return (
    and(
      compileNodeWhere(probe, target, "x", w["node"] as Where | undefined),
      compilePropsWhere(probe, props, "r", w["edge"] as Where | undefined),
    ) !== undefined
  );
}

const AGGREGATE_KEYS = new Set<ListAggregate>([
  "min",
  "max",
  "avg",
  "sum",
  "shortestLength",
  "longestLength",
  "averageLength",
]);

/**
 * `followers: { aggregate: { node: { age: { avg: { gt: 30 } } }, edge: … } }`:
 * aggregates of related values per parent, compared to bounds.
 */
function aggregatePredicate(
  ctx: CompileContext,
  variable: string,
  rel: RelationshipField,
  value: Where,
): Expr | undefined {
  const target = ctx.model.nodes.get(rel.target)!;
  const props = rel.properties
    ? ctx.model.relationshipProperties.get(rel.properties)
    : undefined;
  const parts: Array<Expr | undefined> = [];
  const side = (
    where: Where | undefined,
    lookup: (name: string) => ScalarField | undefined,
    onEdge: boolean,
  ) => {
    for (const [fieldName, aggs] of Object.entries(where ?? {})) {
      const field = lookup(fieldName);
      if (!field || aggs === null || aggs === undefined) continue;
      if (onEdge) checkPropertyAccess(ctx, props!.name, field, "READ");
      else {
        checkFieldAuthentication(ctx, target.name, field);
        // An aggregate over values some rows may not read would reveal
        // them: refused unless the claims alone settle the rules.
        const probe = { ...ctx, params: {}, vars: new Set(ctx.vars) };
        if (fieldValidate(probe, target, field, "probe", "READ")) {
          throw requestError(
            "FORBIDDEN",
            `cannot aggregate ${target.name}.${field.name}: it has row-level read rules`,
          );
        }
        if (!maskSettled(ctx, target, field)) {
          throw requestError(
            "FORBIDDEN",
            `cannot aggregate ${target.name}.${field.name}: it is masked per row`,
          );
        }
      }
      for (const [agg, bounds] of Object.entries(aggs as Where)) {
        if (bounds === null || bounds === undefined) continue;
        if (!AGGREGATE_KEYS.has(agg as ListAggregate)) continue;
        const values = related(
          ctx,
          variable,
          rel,
          () => undefined,
          (x, r) => prop(v(onEdge ? r : x), field.property),
        );
        const aggregate = listAggregate(ctx, agg as ListAggregate, values, {
          duration: field.type === "Duration",
        });
        parts.push(scalarPredicate(ctx, aggregate, bounds as Where));
      }
    }
  };
  side(
    value["node"] as Where | undefined,
    (n) => {
      const f = target.fields.get(n);
      return f?.kind === "scalar" ? f : undefined;
    },
    false,
  );
  if (props) {
    if (value["edge"] != null) refuseEdgeRowRules(ctx, rel);
    side(value["edge"] as Where | undefined, (n) => props.fields.get(n), true);
  }
  if (value["count"]) {
    const total = distinctCount(
      ctx,
      related(
        ctx,
        variable,
        rel,
        () => undefined,
        (x) => prop(v(x), target.key.property),
      ),
    );
    parts.push(compareCount(ctx, rel, total, value["count"] as Where));
  }
  return and(...parts);
}
