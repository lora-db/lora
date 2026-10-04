// Rule sugar, expanded at model build into the rule AST the compiler
// already understands, so a sugared rule compiles to exactly the statement
// its hand-written form does.
//
// - `isViewer: true` on the viewer type (the `@viewer` claim's type) becomes
//   `{ <viewer field>: { eq: "$jwt.<claim>" } }`, at any depth of a `node`
//   filter: `{ node: { author: { isViewer: true } } }`.
// - `{ rule: "name" }` is replaced by that named rule's where: a rule of
//   the type it stands on (`@authorizationRule`), or a claims-only schema
//   rule (`@authorizationRules`). Inside a node filter it names a rule of
//   that node's type and must test `node` only:
//   `{ node: { trip: { rule: "member" } } }`.

import type { ModelProblem } from "../errors.js";
import type {
  AbstractType,
  AuthorizationWhere,
  NodeType,
  RelationshipPropertiesType,
} from "./types.js";

/** The `@viewer` claim: which node, by which unique field, is the caller. */
export interface ViewerMapping {
  claim: string;
  type: string;
  field: string;
}

/** Named rules: claims-only schema rules, and rules per node type. */
export interface NamedRules {
  schema: ReadonlyMap<string, AuthorizationWhere>;
  byType: ReadonlyMap<string, ReadonlyMap<string, AuthorizationWhere>>;
  /** Cycles reported so far, so each is reported once. */
  reportedCycles?: Set<string>;
}

/**
 * Stands where a rule reference could not expand (an error was reported):
 * the rule checks skip it rather than report it again as empty.
 */
export const UNEXPANDED: Readonly<Record<string, never>> = Object.freeze({});

export interface DesugarContext {
  nodes: ReadonlyMap<string, NodeType>;
  props: ReadonlyMap<string, RelationshipPropertiesType>;
  viewer: ViewerMapping | undefined;
  /** Interfaces and unions, for filters through an abstract relationship. */
  abstracts?: ReadonlyMap<string, AbstractType>;
  rules?: NamedRules;
  /** Named rules being expanded, to report a cycle with its chain. */
  expanding?: string[];
  at: (message: string) => void;
}

type Where = Record<string, unknown>;

const isRecord = (x: unknown): x is Where =>
  typeof x === "object" && x !== null && !Array.isArray(x);

const QUANTIFIERS = new Set(["some", "all", "none", "single"]);

/**
 * A rule's where, sugar expanded. `owner` is the type the rule's `node`
 * part tests (undefined on relationship properties: claims only).
 */
export function desugarRule(
  ctx: DesugarContext,
  owner: NodeType | undefined,
  where: AuthorizationWhere,
  ends?: { source: NodeType; target: NodeType },
): AuthorizationWhere {
  if (!isRecord(where)) return where;
  const keys = Object.keys(where);
  if (keys.length === 1 && keys[0] === "rule") {
    // Alone, the reference is its rule: exactly the hand-written form.
    return expandRule(ctx, owner, where["rule"]) ?? UNEXPANDED;
  }
  const out: Where = {};
  const extra: Where[] = [];
  for (const [key, value] of Object.entries(where)) {
    if (key === "rule") {
      const expanded = expandRule(ctx, owner, value);
      if (expanded) extra.push(expanded);
    } else if (key === "AND" || key === "OR") {
      out[key] = Array.isArray(value)
        ? value.map((w) => desugarRule(ctx, owner, w as Where, ends))
        : value;
    } else if (key === "NOT") {
      out[key] = desugarRule(ctx, owner, value as Where, ends);
    } else if (ends && (key === "source" || key === "target")) {
      out[key] = desugarNode(ctx, ends[key], value);
    } else if (key === "node" && owner) {
      out[key] = desugarNode(ctx, owner, value);
    } else if (key === "viewer") {
      const viewerType = ctx.viewer && ctx.nodes.get(ctx.viewer.type);
      if (!viewerType) {
        ctx.at("`viewer` needs a @viewer claim on the @jwt type");
        out[key] = value;
      } else out[key] = desugarNode(ctx, viewerType, value);
    } else {
      out[key] = value;
    }
  }
  if (extra.length > 0) {
    out["AND"] = [...(Array.isArray(out["AND"]) ? out["AND"] : []), ...extra];
  }
  return out;
}

/**
 * The where of the rule `name` names, where it stands: a rule of `owner`
 * (`@authorizationRule`), else a schema rule. Its own references expand
 * too; a cycle is reported with its chain.
 */
function expandRule(
  ctx: DesugarContext,
  owner: NodeType | undefined,
  name: unknown,
): Where | undefined {
  if (typeof name !== "string") {
    ctx.at("`rule` takes the name of a rule");
    return undefined;
  }
  const own = owner && ctx.rules?.byType.get(owner.name)?.get(name);
  const schema = ctx.rules?.schema.get(name);
  const id = own ? `${owner.name}.${name}` : name;
  const def = own ?? schema;
  if (!def) {
    ctx.at(
      `unknown rule "${name}"${owner ? ` (not a rule of ${owner.name} or of the schema)` : " (not a schema rule)"}`,
    );
    return undefined;
  }
  return withinRule(ctx, id, () =>
    desugarRule(ctx, own ? owner : undefined, def),
  );
}

/** Expand a named rule's body, refusing a cycle. */
function withinRule(
  ctx: DesugarContext,
  id: string,
  expand: () => Where,
): Where | undefined {
  const stack = (ctx.expanding ??= []);
  const at = stack.indexOf(id);
  if (at >= 0) {
    const cycle = stack.slice(at);
    const reported = (ctx.rules!.reportedCycles ??= new Set());
    const id_ = [...cycle].sort().join("\0");
    if (!reported.has(id_)) {
      reported.add(id_);
      ctx.at(`rule cycle: ${[...cycle, id].join(" → ")}`);
    }
    return undefined;
  }
  stack.push(id);
  try {
    return expand();
  } finally {
    stack.pop();
  }
}

/**
 * A rule of `node`'s type, used inside a node filter: its `node` parts
 * become the filter itself. A rule that tests anything but `node` (claims,
 * the viewer) cannot stand inside a node filter.
 */
function expandNodeRule(
  ctx: DesugarContext,
  node: NodeType,
  name: unknown,
): unknown {
  if (typeof name !== "string") {
    ctx.at("`rule` takes the name of a rule");
    return undefined;
  }
  const def = ctx.rules?.byType.get(node.name)?.get(name);
  if (!def) {
    ctx.at(
      ctx.rules?.schema.has(name)
        ? `rule "${name}" is a schema rule over claims; use it beside node, not inside it`
        : `unknown rule "${name}" (not a rule of ${node.name})`,
    );
    return undefined;
  }
  const expanded = withinRule(ctx, `${node.name}.${name}`, () =>
    desugarRule(ctx, node, def),
  );
  return expanded && toNodeFilter(ctx, node, name, expanded);
}

function toNodeFilter(
  ctx: DesugarContext,
  node: NodeType,
  name: string,
  where: Where,
): unknown {
  const parts: unknown[] = [];
  for (const [key, value] of Object.entries(where)) {
    if (key === "node" && JSON.stringify(value).includes("${node.")) {
      // Inside another node's filter, `${node.…}` would read the outer node.
      ctx.at(
        `rule ${node.name}.${name} reads \${node.…}; such a rule can't be used inside a node filter, where it would read the outer node`,
      );
    } else if (key === "node") parts.push(value);
    else if (key === "AND" || key === "OR") {
      parts.push({
        [key]: (value as Where[]).map((w) => toNodeFilter(ctx, node, name, w)),
      });
    } else if (key === "NOT") {
      parts.push({ NOT: toNodeFilter(ctx, node, name, value as Where) });
    } else {
      ctx.at(
        `rule ${node.name}.${name} tests ${key}; only a rule that tests node alone can be used inside a node filter`,
      );
    }
  }
  return parts.length === 1 ? parts[0] : { AND: parts };
}

/** A `node` filter over `node`, sugar expanded, keys kept in order. */
export function desugarNode(
  ctx: DesugarContext,
  node: NodeType,
  where: unknown,
): unknown {
  if (!isRecord(where)) return where;
  const keys = Object.keys(where);
  if (keys.length === 1 && keys[0] === "rule") {
    return expandNodeRule(ctx, node, where["rule"]) ?? UNEXPANDED;
  }
  const out: Where = {};
  // Tests that must go under AND, because their key is taken.
  const extra: Where[] = [];
  for (const [key, value] of Object.entries(where)) {
    if (key === "rule") {
      const expanded = expandNodeRule(ctx, node, value);
      if (expanded) extra.push(expanded as Where);
      continue;
    }
    if (key === "AND" || key === "OR") {
      out[key] = Array.isArray(value)
        ? value.map((w) => desugarNode(ctx, node, w))
        : value;
      continue;
    }
    if (key === "NOT") {
      out[key] = desugarNode(ctx, node, value);
      continue;
    }
    if (key === "isViewer") {
      const test = isViewer(ctx, node, value);
      if (test) {
        const [field, ops] = test;
        // The field may be tested too: then both must hold.
        if (field in where) extra.push({ [field]: ops });
        else out[field] = ops;
      }
      continue;
    }
    const f = node.fields.get(key);
    if (f?.kind === "relationship") {
      const target = ctx.nodes.get(f.target);
      const abstract = ctx.abstracts?.get(f.target);
      const inner = (w: unknown) =>
        target
          ? desugarNode(ctx, target, w)
          : abstract
            ? desugarAbstract(ctx, abstract, w)
            : w;
      out[key] = f.list ? quantified(value, inner) : inner(value);
      continue;
    }
    if (!f && key.endsWith("Connection")) {
      const rel = node.fields.get(key.slice(0, -"Connection".length));
      const target =
        rel?.kind === "relationship" ? ctx.nodes.get(rel.target) : undefined;
      if (target) {
        out[key] = quantified(value, (pair) =>
          isRecord(pair) && "node" in pair
            ? { ...pair, node: desugarNode(ctx, target, pair["node"]) }
            : pair,
        );
        continue;
      }
    }
    out[key] = value;
  }
  if (extra.length > 0) {
    out["AND"] = [...(Array.isArray(out["AND"]) ? out["AND"] : []), ...extra];
  }
  return out;
}

/**
 * A filter through a relationship to an interface or union. A union's
 * filter names members (`{ Person: { isViewer: true } }`): each is a node
 * filter of that member. An interface's filter tests the interface's own
 * fields, where node sugar has no single type to expand against.
 */
function desugarAbstract(
  ctx: DesugarContext,
  abstract: AbstractType,
  where: unknown,
): unknown {
  if (!isRecord(where)) return where;
  if (abstract.kind === "union") {
    return Object.fromEntries(
      Object.entries(where).map(([member, w]) => {
        const node = abstract.members.includes(member)
          ? ctx.nodes.get(member)
          : undefined;
        return [member, node ? desugarNode(ctx, node, w) : w];
      }),
    );
  }
  const visit = (w: unknown): void => {
    if (Array.isArray(w)) return w.forEach(visit);
    if (!isRecord(w)) return;
    for (const [k, v] of Object.entries(w)) {
      if (k === "isViewer" || k === "rule") {
        ctx.at(
          `${k} cannot stand in a filter over the interface ${abstract.name}; filter one implementation through a union, or test the viewer field`,
        );
      } else if (k === "AND" || k === "OR" || k === "NOT") visit(v);
    }
  };
  visit(where);
  return where;
}

function quantified(value: unknown, map: (w: unknown) => unknown): unknown {
  if (!isRecord(value)) return value;
  return Object.fromEntries(
    Object.entries(value).map(([q, inner]) => [
      q,
      QUANTIFIERS.has(q) ? map(inner) : inner,
    ]),
  );
}

function isViewer(
  ctx: DesugarContext,
  node: NodeType,
  value: unknown,
): [string, Where] | undefined {
  if (!ctx.viewer) {
    ctx.at("isViewer needs a @viewer claim on the @jwt type");
    return undefined;
  }
  if (value !== true) {
    ctx.at("isViewer takes `true`; wrap it in NOT for the opposite");
    return undefined;
  }
  if (node.name !== ctx.viewer.type) {
    ctx.at(
      `isViewer applies to ${ctx.viewer.type} (the @viewer type), not ${node.name}`,
    );
    return undefined;
  }
  return [ctx.viewer.field, { eq: `$jwt.${ctx.viewer.claim}` }];
}

/** Model problems for a `@viewer` mapping, once the node types are built. */
export function checkViewer(
  viewer: ViewerMapping,
  nodes: ReadonlyMap<string, NodeType>,
  jwtType: string,
  problems: ModelProblem[],
): void {
  const at = (message: string): void => {
    problems.push({ type: jwtType, field: viewer.claim, message });
  };
  const node = nodes.get(viewer.type);
  if (!node) return at(`@viewer: ${viewer.type} is not a @node type`);
  const f = node.fields.get(viewer.field);
  if (f?.kind !== "scalar") {
    return at(`@viewer: ${viewer.type} has no field ${viewer.field}`);
  }
  if (!f.key && !f.unique) {
    at(
      `@viewer: ${viewer.type}.${viewer.field} must be @key or @unique, so it names one node`,
    );
  }
  if (f.list) at(`@viewer: ${viewer.type}.${viewer.field} cannot be a list`);
}

/**
 * The parts a rule tests at its top level, through AND, OR and NOT:
 * `node`, `jwt`, `viewer`, `source`, `target`, `edge`.
 */
export function ruleParts(
  where: unknown,
  out = new Set<string>(),
): Set<string> {
  if (Array.isArray(where)) {
    for (const w of where) ruleParts(w, out);
    return out;
  }
  if (where === null || typeof where !== "object") return out;
  for (const [key, value] of Object.entries(where)) {
    if (key === "AND" || key === "OR" || key === "NOT") ruleParts(value, out);
    else out.add(key);
  }
  return out;
}

/**
 * A relationship property's rule that tests the relationship's ends or
 * edge (`source`, `target`, `edge`): decided per relationship.
 */
export function testsRelationshipEnds(where: unknown): boolean {
  const parts = ruleParts(where);
  return parts.has("source") || parts.has("target") || parts.has("edge");
}
