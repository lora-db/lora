// Startup checks of a rule's where against the model: the keys allowed at
// each level, the fields and operators of node, edge, connection and
// abstract filters, and the claims the @jwt type declares.

import type { ModelProblem } from "../../errors.js";
import { UNEXPANDED } from "../desugar.js";
import type {
  AbstractType,
  NodeType,
  RelationshipField,
  RelationshipPropertiesType,
} from "../types.js";
import { isRecord } from "./authorization.js";
import {
  claimRefs,
  viewerRefProblems,
  ruleReferenceProblems,
  ruleStringProblems,
  type RuleReferences,
} from "./rule-references.js";

const SCALAR_WHERE_OPS = new Set([
  "withinBBox",
  "distance",
  "eq",
  "in",
  "lt",
  "lte",
  "gt",
  "gte",
  "contains",
  "startsWith",
  "endsWith",
]);

const JWT_OPS = new Set([...SCALAR_WHERE_OPS, "includes", "exists"]);
const COUNT_WHERE_OPS = new Set(["eq", "lt", "lte", "gt", "gte"]);

/** Check an `@authorization` where against the model, at startup. */
export function checkAuthorizationWhere(
  nodes: ReadonlyMap<string, NodeType>,
  props: ReadonlyMap<string, RelationshipPropertiesType>,
  node: NodeType,
  where: unknown,
  problems: ModelProblem[],
  jwtShape?: ReadonlyMap<string, string>,
  viewerNode?: NodeType,
) {
  checkRuleWhere(
    nodes,
    props,
    node,
    node.name,
    undefined,
    where,
    problems,
    jwtShape,
    viewerNode,
  );
}

/**
 * Check a rule's where. Without `node` (a relationship property's rules)
 * only claims may be tested: a @relationshipProperties type can sit under
 * several relationship fields, from either end, so `node` would be
 * ambiguous.
 */
/** The ends a relationship field's rule tests, for its checks. */
export interface RuleEnds {
  source: NodeType;
  target: NodeType;
  edge: RelationshipPropertiesType | undefined;
}

export function checkRuleWhere(
  nodes: ReadonlyMap<string, NodeType>,
  props: ReadonlyMap<string, RelationshipPropertiesType>,
  node: NodeType | undefined,
  type: string,
  field: string | undefined,
  where: unknown,
  problems: ModelProblem[],
  jwtShape?: ReadonlyMap<string, string>,
  viewerNode?: NodeType,
  ends?: RuleEnds,
  nodeless = "rules on relationship properties test claims (jwt) only; node rules belong on the node types",
) {
  const at = (message: string) =>
    problems.push({
      type,
      ...(field ? { field } : {}),
      message: `@authorization: ${message}`,
    });
  // What `${node.…}`-style placeholders may read here: the rule's own
  // node, or a relationship rule's ends and edge.
  const references: RuleReferences = ends
    ? { source: ends.source, target: ends.target, edge: ends.edge }
    : node
      ? { node }
      : {};
  // A filter over a node type, with the strings and claims it uses.
  const nodePart = (
    t: NodeType,
    value: unknown,
    here: string,
    refs: RuleReferences = references,
  ) => {
    checkNodeWhere(nodes, props, t, value, here, at);
    for (const problem of ruleStringProblems(value)) at(`${here}: ${problem}`);
    for (const problem of ruleReferenceProblems(value, refs, nodes))
      at(`${here}: ${problem}`);
    for (const problem of viewerRefProblems(value, viewerNode))
      at(`${here}: ${problem}`);
    if (jwtShape) {
      for (const ref of claimRefs(value)) {
        if (!jwtShape.has(ref))
          at(`${here}: $jwt.${ref} is not a claim of the @jwt type`);
      }
    }
  };
  const expected = ends
    ? "source, target, edge, viewer, jwt, AND, OR or NOT"
    : "node, viewer, jwt, AND, OR or NOT";
  const visit = (w: unknown, path: string) => {
    if (w === UNEXPANDED) return; // already reported
    if (!isRecord(w)) return at(`${path || "where"} must be an object`);
    for (const [k, value] of Object.entries(w)) {
      const here = path ? `${path}.${k}` : k;
      if (k === "AND" || k === "OR") {
        if (!Array.isArray(value)) at(`${here} must be a list`);
        else value.forEach((x, i) => visit(x, `${here}[${i}]`));
      } else if (k === "NOT") {
        visit(value, here);
      } else if (ends && (k === "source" || k === "target")) {
        nodePart(ends[k], value, here);
      } else if (ends && k === "edge") {
        if (!ends.edge) at(`${here}: the relationship has no properties`);
        else {
          checkEdgeWhere(ends.edge, value, here, at);
          for (const problem of ruleStringProblems(value))
            at(`${here}: ${problem}`);
          for (const problem of ruleReferenceProblems(value, references, nodes))
            at(`${here}: ${problem}`);
        }
      } else if (k === "node" && ends) {
        at(`${here}: a relationship rule tests source, target and edge`);
      } else if (k === "node" && !node) {
        at(`${here}: ${nodeless}`);
      } else if (k === "node" && node) {
        nodePart(node, value, here);
      } else if (k === "viewer") {
        // Desugaring already reported a missing @viewer. The caller's node
        // is found by the claim alone: nothing of the rule's node to read.
        if (viewerNode) nodePart(viewerNode, value, here, {});
      } else if (k === "jwt") {
        if (!isRecord(value)) at(`${here} must be an object`);
        else {
          for (const [claim, ops] of Object.entries(value)) {
            if (jwtShape && !jwtShape.has(claim)) {
              at(`${here}.${claim}: not a claim of the @jwt type`);
            }
            if (!isRecord(ops))
              at(`${here}.${claim} must be an operator object`);
            else {
              for (const [op, operand] of Object.entries(ops)) {
                if (!JWT_OPS.has(op))
                  at(`${here}.${claim}: unknown operator ${op}`);
                if (JSON.stringify(operand ?? null).includes("${")) {
                  at(
                    `${here}.${claim}.${op}: claim tests compare with literals; \${...} placeholders belong in node parts`,
                  );
                }
              }
            }
          }
        }
      } else {
        at(`${here}: expected ${expected}`);
      }
    }
  };
  visit(where, "");
}

export function checkNodeWhere(
  nodes: ReadonlyMap<string, NodeType>,
  props: ReadonlyMap<string, RelationshipPropertiesType>,
  node: NodeType,
  where: unknown,
  path: string,
  at: (message: string) => void,
) {
  if (where === UNEXPANDED) return; // already reported
  if (!isRecord(where)) return at(`${path} must be an object`);
  // An empty or null test would compile to nothing and grant everyone.
  if (Object.keys(where).length === 0) {
    return at(`${path} is empty: a rule must test something`);
  }
  for (const [k, value] of Object.entries(where)) {
    const here = `${path}.${k}`;
    if (value === null) {
      at(`${here} is null: a rule must test something`);
      continue;
    }
    if (k === "AND" || k === "OR") {
      if (!Array.isArray(value)) at(`${here} must be a list`);
      else
        value.forEach((x, i) =>
          checkNodeWhere(nodes, props, node, x, `${here}[${i}]`, at),
        );
      continue;
    }
    if (k === "NOT") {
      checkNodeWhere(nodes, props, node, value, here, at);
      continue;
    }
    const field = node.fields.get(k);
    const connection = k.endsWith("Connection")
      ? node.fields.get(k.slice(0, -"Connection".length))
      : undefined;
    const exists = k.endsWith("Exists")
      ? node.fields.get(k.slice(0, -"Exists".length))
      : undefined;
    if (!field && exists?.kind === "relationship" && !exists.list) {
      // `<field>Exists: Boolean`: whether the single relationship is set.
      if (typeof value !== "boolean") at(`${here} must be true or false`);
    } else if (
      !field &&
      connection?.kind === "relationship" &&
      connection.list &&
      connection.properties
    ) {
      // `<field>Connection: { some: { node, edge } }`, as in a public where.
      checkConnectionWhere(nodes, props, connection, value, here, at);
    } else if (!field) {
      at(`${here}: ${node.name} has no field ${k}`);
    } else if (field.kind === "cypher" || field.kind === "custom") {
      at(
        `${here}: ${field.kind === "cypher" ? "@cypher" : "@customResolver"} fields cannot be used in rules`,
      );
    } else if (field.kind === "scalar") {
      if (!isRecord(value) || Object.keys(value).length === 0) {
        at(`${here} must be a non-empty operator object`);
      } else {
        for (const [op, operand] of Object.entries(value)) {
          if (!SCALAR_WHERE_OPS.has(op)) at(`${here}: unknown operator ${op}`);
          else if (operand === null) at(`${here}.${op} is null`);
        }
      }
    } else {
      const target = nodes.get(field.target);
      const abstract = abstractsOf.get(nodes)?.get(field.target);
      const check = (w: unknown, path: string) =>
        target
          ? checkNodeWhere(nodes, props, target, w, path, at)
          : abstract
            ? checkAbstractWhere(nodes, props, abstract, w, path, at)
            : undefined;
      if (!target && !abstract) continue;
      if (!field.list) {
        check(value, here);
        continue;
      }
      if (!isRecord(value)) {
        at(`${here} must be an object`);
        continue;
      }
      for (const [q, inner] of Object.entries(value)) {
        if (q === "count") {
          if (!isRecord(inner)) at(`${here}.count must be an operator object`);
          else {
            for (const op of Object.keys(inner)) {
              if (!COUNT_WHERE_OPS.has(op))
                at(`${here}.count: unknown operator ${op}`);
            }
          }
        } else if (["some", "all", "none", "single"].includes(q)) {
          check(inner, `${here}.${q}`);
        } else {
          at(`${here}: expected some, all, none, single or count`);
        }
      }
    }
  }
}

/**
 * Interfaces and unions of a model being built, by its node map: the rule
 * checks below receive the node map only.
 */
export const abstractsOf = new WeakMap<
  ReadonlyMap<string, NodeType>,
  ReadonlyMap<string, AbstractType>
>();

/**
 * A rule's filter through a relationship to an interface or union: a
 * union's names members, each a node filter of that member; an
 * interface's tests its own fields and `typename`.
 */
function checkAbstractWhere(
  nodes: ReadonlyMap<string, NodeType>,
  props: ReadonlyMap<string, RelationshipPropertiesType>,
  abstract: AbstractType,
  where: unknown,
  path: string,
  at: (message: string) => void,
) {
  if (where === UNEXPANDED) return; // already reported
  if (!isRecord(where) || Object.keys(where).length === 0) {
    return at(`${path} is empty: a rule must test something`);
  }
  for (const [k, value] of Object.entries(where)) {
    const here = `${path}.${k}`;
    if (value === null) {
      at(`${here} is null: a rule must test something`);
      continue;
    }
    if (abstract.kind === "union") {
      const member = abstract.members.includes(k) ? nodes.get(k) : undefined;
      if (!member) at(`${here}: ${k} is not a member of ${abstract.name}`);
      else checkNodeWhere(nodes, props, member, value, here, at);
      continue;
    }
    if (k === "AND" || k === "OR") {
      if (!Array.isArray(value)) at(`${here} must be a list`);
      else
        value.forEach((x, i) =>
          checkAbstractWhere(nodes, props, abstract, x, `${here}[${i}]`, at),
        );
    } else if (k === "NOT") {
      checkAbstractWhere(nodes, props, abstract, value, here, at);
    } else if (k === "typename") {
      if (
        !Array.isArray(value) ||
        value.some((m) => !abstract.members.includes(m as string))
      ) {
        at(`${here}: a list of ${abstract.name}'s implementations`);
      }
    } else if (!abstract.fields.has(k)) {
      at(`${here}: ${abstract.name} has no field ${k}`);
    } else if (!isRecord(value) || Object.keys(value).length === 0) {
      at(`${here} must be a non-empty operator object`);
    } else {
      for (const [op, operand] of Object.entries(value)) {
        if (!SCALAR_WHERE_OPS.has(op)) at(`${here}: unknown operator ${op}`);
        else if (operand === null) at(`${here}.${op} is null`);
      }
    }
  }
}

/** A rule's `<field>Connection` test: quantifiers over { node, edge }. */
function checkConnectionWhere(
  nodes: ReadonlyMap<string, NodeType>,
  props: ReadonlyMap<string, RelationshipPropertiesType>,
  rel: RelationshipField,
  value: unknown,
  path: string,
  at: (message: string) => void,
) {
  if (!isRecord(value) || Object.keys(value).length === 0) {
    return at(`${path} must be a non-empty object`);
  }
  const target = nodes.get(rel.target);
  const edgeType = props.get(rel.properties!);
  for (const [q, inner] of Object.entries(value)) {
    const here = `${path}.${q}`;
    if (!["some", "all", "none", "single"].includes(q)) {
      at(`${path}: expected some, all, none or single`);
      continue;
    }
    if (!isRecord(inner) || Object.keys(inner).length === 0) {
      at(`${here} is empty: a rule must test something`);
      continue;
    }
    for (const [side, test] of Object.entries(inner)) {
      if (side === "node") {
        if (target)
          checkNodeWhere(nodes, props, target, test, `${here}.node`, at);
      } else if (side === "edge") {
        if (edgeType) checkEdgeWhere(edgeType, test, `${here}.edge`, at);
      } else {
        at(`${here}: expected node or edge`);
      }
    }
  }
}

function checkEdgeWhere(
  edgeType: RelationshipPropertiesType,
  where: unknown,
  path: string,
  at: (message: string) => void,
) {
  if (!isRecord(where) || Object.keys(where).length === 0) {
    return at(`${path} is empty: a rule must test something`);
  }
  for (const [k, value] of Object.entries(where)) {
    const here = `${path}.${k}`;
    if (value === null) {
      at(`${here} is null: a rule must test something`);
    } else if (k === "AND" || k === "OR") {
      if (!Array.isArray(value)) at(`${here} must be a list`);
      else
        value.forEach((x, i) =>
          checkEdgeWhere(edgeType, x, `${here}[${i}]`, at),
        );
    } else if (k === "NOT") {
      checkEdgeWhere(edgeType, value, here, at);
    } else if (!edgeType.fields.has(k)) {
      at(`${here}: ${edgeType.name} has no field ${k}`);
    } else if (!isRecord(value) || Object.keys(value).length === 0) {
      at(`${here} must be a non-empty operator object`);
    } else {
      for (const [op, operand] of Object.entries(value)) {
        if (!SCALAR_WHERE_OPS.has(op)) at(`${here}: unknown operator ${op}`);
        else if (operand === null) at(`${here}.${op} is null`);
      }
    }
  }
}
