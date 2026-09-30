// What the generated mutation inputs offer, decided from the model alone,
// so an input type that would have no field is left out (with whatever
// would take it) instead of breaking the schema.

import { memberFields } from "./relations.js";
import type {
  GraphModel,
  NodeType,
  RelationshipField,
  RelationshipPropertiesType,
  ScalarField,
} from "./types.js";

/** Whether clients may set a field on `op` (`@settable`, `@readonly`, …). */
export function settable(
  f: ScalarField | RelationshipField,
  op: "CREATE" | "UPDATE",
): boolean {
  return op === "CREATE" ? f.settableOn.create : f.settableOn.update;
}

/**
 * Whether an update input for `node` has any field: a scalar settable on
 * update, or a relationship offering a nested operation.
 */
export function isUpdatable(
  model: GraphModel,
  node: NodeType,
  seen: Set<string> = new Set(),
): boolean {
  if (seen.has(node.name)) return false;
  seen.add(node.name);
  return [...node.fields.values()].some((f) =>
    f.kind === "scalar"
      ? settable(f, "UPDATE")
      : f.kind === "relationship" &&
        settable(f, "UPDATE") &&
        offersNested(model, f, true, seen),
  );
}

/** Whether the create or update input of `rel` offers any operation. */
export function offersNested(
  model: GraphModel,
  rel: RelationshipField,
  update: boolean,
  seen: Set<string> = new Set(),
): boolean {
  const allows = (op: Parameters<typeof rel.nestedOperations.has>[0]) =>
    rel.nestedOperations.has(op);
  const members = memberFields(model, rel);
  if (allows("CONNECT") && members.length > 0) return true;
  if (
    allows("CREATE") &&
    members.some((m) => model.nodes.get(m.target)?.mutations.has("CREATE"))
  ) {
    return true;
  }
  if (!update) return false;
  if (allows("DISCONNECT") && members.length > 0) return true;
  // Interfaces and unions offer connect, create and disconnect only.
  if (model.abstracts.has(rel.target)) return false;
  const target = model.nodes.get(rel.target)!;
  if (allows("DELETE") && target.mutations.has("DELETE")) return true;
  if (!allows("UPDATE")) return false;
  const props = rel.properties
    ? model.relationshipProperties.get(rel.properties)
    : undefined;
  return (
    (props !== undefined && hasSettable(props, "UPDATE")) ||
    (target.mutations.has("UPDATE") && isUpdatable(model, target, seen))
  );
}

/** Whether any property of a @relationshipProperties type is settable on `op`. */
export function hasSettable(
  props: RelationshipPropertiesType,
  op: "CREATE" | "UPDATE",
): boolean {
  return [...props.fields.values()].some((f) => settable(f, op));
}
