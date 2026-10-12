// Errors mutations raise for what the graph holds: a pair that is not
// connected, a target that is not there, a relationship rule that
// refuses, a single relationship holding two, a required relationship
// left empty, and the engine's constraint errors named by field.

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

/** The error of a connect whose targets are not there (or not visible). */
export function noTarget(
  owner: NodeType,
  rel: RelationshipField,
  target: NodeType,
  missing: unknown[],
) {
  return requestError(
    "NOT_FOUND",
    `${owner.name}.${rel.name}: no ${target.name} with ${target.key.name} ${missing
      .map((k) => JSON.stringify(k))
      .join(", ")}`,
    undefined,
    { type: target.name, keys: missing },
  );
}

/** The error of a relationship field's rules refusing `op`. */
export function refusedRule(
  op: "CONNECT" | "DISCONNECT" | "UPDATE_EDGE",
  rel: RelationshipField,
) {
  return requestError(
    "FORBIDDEN",
    `not allowed to ${op.toLowerCase().replace("_", " ")} ${rel.owner}.${rel.name}`,
  );
}

/** The error of a single relationship field that would hold `count` nodes. */
export function tooMany(
  field: RelationshipField,
  key: unknown,
  count: number,
  seen: number,
) {
  // Held by a node the caller cannot read: replacing it would write
  // a relationship of a node they cannot see, and the count would
  // reveal it. Refused like a rule would refuse it.
  if (seen < count) {
    return requestError(
      "FORBIDDEN",
      `not allowed to replace ${field.owner}.${field.name}`,
    );
  }
  return requestError(
    "CONSTRAINT_VIOLATION",
    `${field.owner}.${field.name} holds one ${field.target}, but ${field.owner} ${JSON.stringify(key)} would have ${String(count)}`,
    undefined,
    { type: field.owner, field: field.name },
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
