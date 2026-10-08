// The value rules of a mutation's input (@normalize, @length, @pattern,
// @format, @range, @multipleOf, @size, and the validators option),
// applied in JavaScript before the transaction opens: a request with a
// bad value never takes the writer lock. Walks the arguments the way the
// plan does (create and update inputs, nested connect / create / update,
// relationship properties, the member-keyed parts of interface and union
// inputs) and reports every violation at once.
//
// Order: normalise, then the field rules, then the type's validator.
// What is normalised is written back into the arguments, so the plan
// stores the value that was checked. That is safe to repeat: when two
// root fields share a variable's value, normalising it twice changes
// nothing.
//
// Keys are checked where they are stored (a create), never where they
// address a node (connect, disconnect, the key of an update): a rule
// added later must not make an existing node unreachable.

import {
  MAX_VIOLATIONS,
  validationError,
  type Violation,
} from "../../errors.js";
import { checkValue, normalizeValue } from "../../model/constraints.js";
import type {
  GraphModel,
  NodeType,
  RelationshipField,
  ScalarField,
} from "../../model/types.js";
import type { MutationKind } from "../../schema/mutations.js";
import {
  isStandardSchema,
  type StandardSchemaV1,
  type TypeValidator,
} from "../../standard-schema.js";
import type { Input } from "./plan.js";

type Path = Array<string | number>;
type InputKind = "create" | "update" | "upsert";
export type Validators = Readonly<Record<string, TypeValidator>>;

interface Job {
  schema: StandardSchemaV1;
  node: NodeType;
  input: Input;
  path: Path;
}

class Collector {
  readonly violations: Violation[] = [];
  readonly jobs: Job[] = [];
  total = 0;

  constructor(
    readonly model: GraphModel,
    readonly validators: Validators,
  ) {}

  add(violation: Violation): void {
    this.total++;
    if (this.violations.length < MAX_VIOLATIONS)
      this.violations.push(violation);
  }

  /** Normalise `holder[f.name]` in place, then apply the field's rules. */
  field(owner: string, f: ScalarField, holder: Input, path: Path): void {
    if (!f.constraints) return;
    let value = holder[f.name];
    if (value === null || value === undefined) return;
    if (f.constraints.normalize) {
      value = normalizeValue(f.constraints, value);
      holder[f.name] = value;
    }
    for (const failure of checkValue(f.constraints, value)) {
      this.add({
        path:
          failure.index === undefined
            ? [...path, f.name]
            : [...path, f.name, failure.index],
        coordinate: `${owner}.${f.name}`,
        rule: failure.rule,
        params: failure.params,
        message: failure.message,
      });
    }
  }
}

function schemaFor(
  validator: TypeValidator | undefined,
  kind: InputKind,
): StandardSchemaV1 | undefined {
  if (validator === undefined) return undefined;
  return isStandardSchema(validator) ? validator : validator[kind];
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
  rel: RelationshipField,
  edge: unknown,
  path: Path,
  out: Collector,
): void {
  if (!rel.properties || edge === null || typeof edge !== "object") return;
  const props = out.model.relationshipProperties.get(rel.properties);
  if (!props) return;
  for (const f of props.fields.values()) {
    out.field(props.name, f, edge as Input, path);
  }
}

function walkRelationship(
  rel: RelationshipField,
  value: Input,
  path: Path,
  out: Collector,
): void {
  const { model } = out;
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
      walkEdge(rel, item["edge"], [...itemPath, "edge"], out);
    }
  }
  for (const [target, given, at] of parts("create")) {
    for (const [item, itemPath] of items(given, at)) {
      walkEdge(rel, item["edge"], [...itemPath, "edge"], out);
      walkNode(target, item["node"], [...itemPath, "node"], "create", out);
    }
  }
  for (const [target, given, at] of parts("update")) {
    for (const [item, itemPath] of items(given, at)) {
      walkEdge(rel, item["edge"], [...itemPath, "edge"], out);
      walkNode(target, item["node"], [...itemPath, "node"], "update", out);
    }
  }
}

function walkNode(
  node: NodeType,
  input: unknown,
  path: Path,
  kind: InputKind,
  out: Collector,
): void {
  if (input === null || typeof input !== "object") return;
  const holder = input as Input;
  for (const f of node.fields.values()) {
    const value = holder[f.name];
    if (value === null || value === undefined) continue;
    if (f.kind === "scalar") {
      out.field(node.name, f, holder, path);
    } else if (f.kind === "relationship" && typeof value === "object") {
      walkRelationship(f, value as Input, [...path, f.name], out);
    }
  }
  const schema = schemaFor(out.validators[node.name], kind);
  if (schema) out.jobs.push({ schema, node, input: holder, path });
}

// Dates and times travel to the database as { kind, iso }. A validator is
// shown what the client sent, an ISO string, and a string it returns for
// such a field is given its kind back.
const TEMPORAL: Partial<Record<ScalarField["type"], string>> = {
  Date: "date",
  Time: "time",
  LocalTime: "localtime",
  DateTime: "datetime",
  LocalDateTime: "localdatetime",
  Duration: "duration",
};

const mapItems = (value: unknown, each: (item: unknown) => unknown) =>
  Array.isArray(value) ? value.map(each) : each(value);

function temporalFields(node: NodeType): ScalarField[] {
  return [...node.fields.values()].filter(
    (f): f is ScalarField => f.kind === "scalar" && f.type in TEMPORAL,
  );
}

/** Run one type's validator; its issues become violations at its path. */
async function runValidator(job: Job, out: Collector): Promise<void> {
  const { schema, node, input, path } = job;
  const temporal = temporalFields(node);
  const shown: Input = { ...input };
  for (const f of temporal) {
    shown[f.name] = mapItems(input[f.name], (item) =>
      item !== null && typeof item === "object" && "iso" in item
        ? (item as { iso: unknown }).iso
        : item,
    );
  }
  const result = await schema["~standard"].validate(shown);
  if (result.issues) {
    for (const issue of result.issues) {
      // A segment is a key or an object holding one, and a library may
      // give a list index as a string.
      const keys = (issue.path ?? []).map((segment) => {
        const key =
          segment !== null && typeof segment === "object"
            ? segment.key
            : segment;
        return typeof key === "number" ? key : String(key);
      });
      const first = keys[0];
      out.add({
        path: [...path, ...keys],
        coordinate:
          typeof first === "string" && node.fields.has(first)
            ? `${node.name}.${first}`
            : node.name,
        rule: "custom",
        params: {},
        message: issue.message,
      });
    }
    return;
  }
  // A validator may transform: what it returns is what is written.
  const value: unknown = result.value;
  if (value === null || typeof value !== "object") return;
  for (const key of Object.keys(input)) delete input[key];
  Object.assign(input, value);
  for (const f of temporal) {
    input[f.name] = mapItems(input[f.name], (item) =>
      typeof item === "string" ? { kind: TEMPORAL[f.type], iso: item } : item,
    );
  }
}

/**
 * Refuse a mutation whose input breaks a field's rule or its type's
 * validator, with every violation in one `BAD_USER_INPUT`. Values
 * computed during the write (an `adjust`, a `@populatedBy` callback) are
 * checked where they appear.
 */
export async function validateInput(
  model: GraphModel,
  validators: Validators,
  op: MutationKind,
  node: NodeType,
  args: Record<string, unknown>,
): Promise<void> {
  const out = new Collector(model, validators);
  if (op === "CREATE" || op === "UPSERT") {
    const kind = op === "CREATE" ? "create" : "upsert";
    for (const [input, path] of items(args["input"], ["input"])) {
      walkNode(node, input, path, kind, out);
    }
  } else if (op === "UPDATE" || op === "UPDATE_MANY") {
    walkNode(node, args["update"], ["update"], "update", out);
  }
  // A validator sees an input the field rules accept: after them, and
  // only when they all hold, so it need not defend against what they refuse.
  if (out.total === 0) {
    for (const job of out.jobs) await runValidator(job, out);
  }
  if (out.total > 0) {
    throw validationError("BAD_USER_INPUT", out.violations, out.total);
  }
}
