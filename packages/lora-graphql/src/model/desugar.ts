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
}

export interface DesugarContext {
  nodes: ReadonlyMap<string, NodeType>;
  props: ReadonlyMap<string, RelationshipPropertiesType>;
  viewer: ViewerMapping | undefined;
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
    return expandRule(ctx, owner, where["rule"]) ?? {};
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
    ctx.at(`rule cycle: ${[...stack.slice(at), id].join(" → ")}`);
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
    if (key === "node") parts.push(value);
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
    return expandNodeRule(ctx, node, where["rule"]) ?? {};
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
      if (!target) out[key] = value;
      else if (!f.list) out[key] = desugarNode(ctx, target, value);
      else out[key] = quantified(value, (w) => desugarNode(ctx, target, w));
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
