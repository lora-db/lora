// The value rules of a mutation's input (@length, @pattern, @format,
// @range, @multipleOf, @size), checked in JavaScript before the
// transaction opens: a request with a bad value never takes the writer
// lock. Walks the arguments the way the plan does (create and update
// inputs, nested connect / create / update, relationship properties, the
// member-keyed parts of interface and union inputs) and reports every
// violation at once.
//
// Keys are checked where they are stored (a create), never where they
// address a node (connect, disconnect, the key of an update): a rule
// added later must not make an existing node unreachable.

import {
  MAX_VIOLATIONS,
  validationError,
  type Violation,
} from "../../errors.js";
import { checkValue } from "../../model/constraints.js";
import type {
  GraphModel,
  NodeType,
  RelationshipField,
  ScalarField,
} from "../../model/types.js";
import type { MutationKind } from "../../schema/mutations.js";
import type { Input } from "./plan.js";

type Path = Array<string | number>;

class Collector {
  readonly violations: Violation[] = [];
  total = 0;

  check(owner: string, f: ScalarField, value: unknown, path: Path): void {
    if (!f.constraints) return;
    for (const failure of checkValue(f.constraints, value)) {
      this.total++;
      if (this.violations.length >= MAX_VIOLATIONS) continue;
      this.violations.push({
        path: failure.index === undefined ? path : [...path, failure.index],
        coordinate: `${owner}.${f.name}`,
        rule: failure.rule,
        params: failure.params,
        message: failure.message,
      });
    }
  }
}

/** Items of a nested operation with their paths: one, or a list. */
function* items(value: unknown, path: Path): Generator<[Input, Path]> {
  if (value === null || value === undefined) return;
  if (Array.isArray(value)) {
    for (const [i, item] of value.entries()) {
      if (item !== null && typeof item === "object") {
        yield [item as Input, [...path, i]];
      }
    }
  } else if (typeof value === "object") {
    yield [value as Input, path];
  }
}

function walkEdge(
  model: GraphModel,
  rel: RelationshipField,
  edge: unknown,
  path: Path,
  out: Collector,
): void {
  if (!rel.properties || edge === null || typeof edge !== "object") return;
  const props = model.relationshipProperties.get(rel.properties);
  if (!props) return;
  for (const f of props.fields.values()) {
    out.check(props.name, f, (edge as Input)[f.name], [...path, f.name]);
  }
}

function walkRelationship(
  model: GraphModel,
  rel: RelationshipField,
  value: Input,
  path: Path,
  out: Collector,
): void {
  const parts = (op: string): Array<[NodeType, unknown, Path]> => {
    const given = value[op];
    if (given === null || given === undefined) return [];
    if (!model.abstracts.has(rel.target)) {
      const target = model.nodes.get(rel.target);
      return target ? [[target, given, [...path, op]]] : [];
    }
    // An interface or union target takes its parts per member type.
    return Object.entries(given as Input).flatMap(([member, part]) => {
      const target = model.nodes.get(member);
      return target
        ? [[target, part, [...path, op, member]] as [NodeType, unknown, Path]]
        : [];
    });
  };
  for (const [, given, at] of parts("connect")) {
    for (const [item, itemPath] of items(given, at)) {
      walkEdge(model, rel, item["edge"], [...itemPath, "edge"], out);
    }
  }
  for (const [target, given, at] of parts("create")) {
    for (const [item, itemPath] of items(given, at)) {
      walkEdge(model, rel, item["edge"], [...itemPath, "edge"], out);
      walkNode(model, target, item["node"], [...itemPath, "node"], out);
    }
  }
  for (const [target, given, at] of parts("update")) {
    for (const [item, itemPath] of items(given, at)) {
      walkEdge(model, rel, item["edge"], [...itemPath, "edge"], out);
      walkNode(model, target, item["node"], [...itemPath, "node"], out);
    }
  }
}

function walkNode(
  model: GraphModel,
  node: NodeType,
  input: unknown,
  path: Path,
  out: Collector,
): void {
  if (input === null || typeof input !== "object") return;
  for (const f of node.fields.values()) {
    const value = (input as Input)[f.name];
    if (value === null || value === undefined) continue;
    if (f.kind === "scalar") {
      out.check(node.name, f, value, [...path, f.name]);
    } else if (f.kind === "relationship" && typeof value === "object") {
      walkRelationship(model, f, value as Input, [...path, f.name], out);
    }
  }
}

/**
 * Refuse a mutation whose input breaks a field's rule, with every
 * violation in one `BAD_USER_INPUT`. Values computed during the write (an
 * `adjust`, a `@populatedBy` callback) are checked where they appear.
 */
export function validateInput(
  model: GraphModel,
  op: MutationKind,
  node: NodeType,
  args: Record<string, unknown>,
): void {
  const out = new Collector();
  if (op === "CREATE" || op === "UPSERT") {
    for (const [input, path] of items(args["input"], ["input"])) {
      walkNode(model, node, input, path, out);
    }
  } else if (op === "UPDATE" || op === "UPDATE_MANY") {
    walkNode(model, node, args["update"], ["update"], out);
  }
  if (out.total > 0) {
    throw validationError("BAD_USER_INPUT", out.violations, out.total);
  }
}
