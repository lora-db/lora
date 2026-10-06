// Errors mutations raise for what the graph holds: a pair that is not
// connected, a required relationship left empty, and the engine's
// constraint errors named by field.

import { requestError } from "../../errors.js";
import type {
  GraphModel,
  NodeType,
  RelationshipField,
  ScalarField,
} from "../../model/types.js";

export function notConnected(
  owner: NodeType,
  rel: RelationshipField,
  target: NodeType,
  to: unknown,
) {
  return requestError(
    "NOT_FOUND",
    `${owner.name}.${rel.name}: ${to === undefined ? `no ${target.name} is connected` : `${target.name} ${JSON.stringify(to)} is not connected`}`,
    undefined,
    { type: target.name },
  );
}

/** In place of the key of a node the caller cannot read, in messages. */
export const HIDDEN = Symbol("hidden");

export function requiredMissing(
  owner: NodeType,
  field: RelationshipField,
  target: string,
  key: unknown,
) {
  const who =
    key === HIDDEN
      ? `a ${owner.name} the caller can't read`
      : `${owner.name} ${JSON.stringify(key)}`;
  return requestError(
    "CONSTRAINT_VIOLATION",
    `${who} requires a ${target} (${owner.name}.${field.name})`,
    undefined,
    { type: owner.name, field: field.name },
  );
}

/** Turn engine constraint errors into CONSTRAINT_VIOLATION naming the field. */
export function mapWriteError(model: GraphModel, err: unknown): unknown {
  const code = (err as { code?: string }).code;
  const message = err instanceof Error ? err.message : String(err);
  if (
    code !== "LORA_UNIQUE_CONSTRAINT" &&
    code !== "LORA_NOT_NULL_CONSTRAINT"
  ) {
    return err;
  }
  const label = /label `([^`]+)`/.exec(message)?.[1];
  const property = /property `([^`]+)`/.exec(message)?.[1];
  const node = [...model.nodes.values()].find((n) => n.labels[0] === label);
  const field = node
    ? [...node.fields.values()].find(
        (f): f is ScalarField => f.kind === "scalar" && f.property === property,
      )
    : undefined;
  const what =
    node && field ? `${node.name}.${field.name}` : `${label}.${property}`;
  return requestError(
    "CONSTRAINT_VIOLATION",
    code === "LORA_UNIQUE_CONSTRAINT"
      ? `${what} must be unique; the value is taken`
      : `${what} is required`,
    err,
    { type: node?.name ?? label, field: field?.name ?? property },
  );
}
