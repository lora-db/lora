// A stored field of a @node or @relationshipProperties type: its storage
// type, key, filters, indexes, defaults and computed values, each checked
// against the directives that may stand next to it.

import {
  getNamedType,
  isEnumType,
  isScalarType,
  Kind,
  type GraphQLDirective,
  type GraphQLField,
  type GraphQLObjectType,
} from "graphql";
import { RANGE_UNINDEXABLE } from "../../analyze/indexes.js";
import type { ModelProblem } from "../../errors.js";
import type {
  AuthOperation,
  AuthorizationWhere,
  FilterOperator,
  IndexKind,
  ScalarField,
  ScalarType,
} from "../types.js";
import { BUILTIN_SCALARS, unwrap, storageOf } from "./shapes.js";
import { directive } from "./directive-args.js";
import { readAuthorization } from "./authorization.js";
import { vectorQueryNames } from "./search.js";

const TEXT_OPS: FilterOperator[] = ["CONTAINS", "STARTS_WITH", "ENDS_WITH"];
const RANGE_OPS: FilterOperator[] = ["LT", "LTE", "GT", "GTE"];

/** Which filter operators each stored type supports. */
const ALLOWED_OPS: Record<ScalarType, readonly FilterOperator[]> = {
  String: ["EQ", "IN", ...RANGE_OPS, ...TEXT_OPS],
  ID: ["EQ", "IN", ...RANGE_OPS, ...TEXT_OPS],
  Int: ["EQ", "IN", ...RANGE_OPS],
  Float: ["EQ", "IN", ...RANGE_OPS],
  BigInt: ["EQ", "IN", ...RANGE_OPS],
  Boolean: ["EQ"],
  Date: ["EQ", "IN", ...RANGE_OPS],
  Time: ["EQ", "IN", ...RANGE_OPS],
  LocalTime: ["EQ", "IN", ...RANGE_OPS],
  DateTime: ["EQ", "IN", ...RANGE_OPS],
  LocalDateTime: ["EQ", "IN", ...RANGE_OPS],
  Duration: ["EQ", "IN"],
  Enum: ["EQ", "IN"],
  Point: ["WITHIN_BBOX", "DISTANCE"],
  CartesianPoint: ["WITHIN_BBOX", "DISTANCE"],
};

const KEY_TYPES = new Set<ScalarType>(["String", "ID", "Int", "BigInt"]);
const UNSORTABLE = new Set<ScalarType>(["Point", "CartesianPoint", "Duration"]);

export function buildScalarField(
  t: GraphQLObjectType,
  f: GraphQLField<unknown, unknown>,
  d: (n: string) => GraphQLDirective,
  problems: ModelProblem[],
  opts: { allowKey: boolean },
): ScalarField | undefined {
  const at = (message: string) =>
    problems.push({ type: t.name, field: f.name, message });
  const shape = unwrap(f.type);
  const named = getNamedType(f.type);
  let type: ScalarType;
  let enumName: string | undefined;
  if (isEnumType(named)) {
    type = "Enum";
    enumName = named.name;
  } else if (named.name in BUILTIN_SCALARS) {
    type = BUILTIN_SCALARS[named.name]!;
  } else if (storageOf(named)) {
    type = storageOf(named)!;
  } else {
    return undefined;
  }
  const customScalar =
    isScalarType(named) && !(named.name in BUILTIN_SCALARS)
      ? named.name
      : undefined;
  if (shape.list && !shape.itemRequired) {
    at("list items must be non-null, e.g. [String!]");
  }
  if (shape.depth > 1) at("nested lists are not supported");

  const key = directive(d("key"), f, at) !== undefined;
  const unique = directive(d("unique"), f, at) !== undefined;
  const isPrivate = directive(d("private"), f, at) !== undefined;
  const relayId = directive(d("relayId"), f, at) !== undefined;
  const sortable = directive(d("sortable"), f, at) !== undefined;
  const groupBy = directive(d("groupBy"), f, at) !== undefined;
  const filterable = directive(d("filterable"), f, at);
  const alias = directive(d("alias"), f, at);
  const index = directive(d("index"), f, at);
  const defaultArgs = directive(d("default"), f, at);
  const timestampArgs = directive(d("timestamp"), f, at);
  const readonlyFlag = directive(d("readonly"), f, at) !== undefined;
  const authentication = directive(d("authentication"), f, at);
  const settableArgs = directive(d("settable"), f, at);
  const selectableArgs = directive(d("selectable"), f, at);
  const populatedArgs = directive(d("populatedBy"), f, at);
  const fieldAuthorization = readAuthorization(
    directive(d("authorization"), f, at),
  );
  if (key && (settableArgs || selectableArgs || populatedArgs)) {
    at("@key cannot take @settable, @selectable or @populatedBy");
  }
  const vectorArgs = directive(d("vector"), f, at);
  let vector: ScalarField["vector"];
  if (vectorArgs) {
    const dimensions = vectorArgs["dimensions"] as number;
    if (type !== "Float" || !shape.list) at("@vector needs a [Float!] field");
    if (!opts.allowKey) at("@vector is not allowed on relationship properties");
    if (!(dimensions >= 1 && dimensions <= 4096)) {
      at("@vector(dimensions:) must be between 1 and 4096");
    }
    vector = {
      dimensions,
      similarity: vectorArgs["similarity"] as "COSINE" | "EUCLIDEAN",
    };
  }
  const vectorQuery = vectorArgs?.["queryName"] as string | undefined;
  const generate = key && (keyArgs(d, f, at)?.["generate"] as boolean) === true;
  const scopeArgs = key ? keyArgs(d, f, at) : undefined;
  const keyScope =
    scopeArgs?.["scope"] === "VIEWER"
      ? { separator: (scopeArgs["separator"] as string | undefined) ?? ":" }
      : undefined;
  if (keyScope) {
    if (generate) at("@key(scope:) and @key(generate: true) are exclusive");
    if (keyScope.separator.length === 0) {
      at("@key(separator:) cannot be empty");
    }
    if (type !== "ID" && type !== "String") {
      at("@key(scope:) needs an ID or String field");
    }
  }
  if (generate && type !== "ID" && type !== "String") {
    at("@key(generate: true) needs an ID or String field");
  }
  const timestamp = timestampArgs
    ? new Set(timestampArgs["operations"] as Array<"CREATE" | "UPDATE">)
    : undefined;
  if (
    timestamp &&
    (shape.list ||
      (type !== "DateTime" && type !== "LocalDateTime" && type !== "Date"))
  ) {
    at("@timestamp needs a DateTime, LocalDateTime or Date field");
  }
  if (timestamp && defaultArgs) at("@timestamp and @default are exclusive");
  let defaultValue: { value: unknown } | undefined;
  if (defaultArgs) {
    const value = defaultArgs["value"];
    if (!defaultMatches(type, shape.list, value)) {
      at(`@default(value:) does not fit ${shape.list ? `[${type}]` : type}`);
    }
    if (key) at("@key cannot have a @default; use @key(generate: true)");
    if (type === "Point" || type === "CartesianPoint") {
      at("Point fields cannot have a @default");
    }
    defaultValue = { value };
  }

  if (!opts.allowKey && (key || unique || relayId || index)) {
    at("@key, @unique, @relayId and @index are not allowed here");
  }
  if (key) {
    if (!KEY_TYPES.has(type)) at("@key must be String, ID, Int or BigInt");
    if (!shape.required) at("@key fields must be non-null");
    if (shape.list) at("@key cannot be a list");
    if (isPrivate) at("@key cannot be @private");
  }
  if (relayId && !key) at("@relayId belongs on the @key field");
  if (unique && shape.list) at("@unique cannot be a list");

  let filters = new Set<FilterOperator>();
  if (filterable) {
    // Lists: membership and presence. Scalars: the type's operators, plus
    // IS_NULL when nullable and CASE_INSENSITIVE on strings.
    const allowed: FilterOperator[] = shape.list
      ? type === "Point" || type === "CartesianPoint" || vectorArgs
        ? []
        : ["INCLUDES", ...(shape.required ? [] : (["IS_NULL"] as const))]
      : [
          ...ALLOWED_OPS[type],
          ...(shape.required ? [] : (["IS_NULL"] as const)),
          ...(type === "String" || type === "ID"
            ? (["CASE_INSENSITIVE"] as const)
            : []),
        ];
    // Without byValue: equality, and membership where the type has it.
    const requested =
      (filterable["byValue"] as FilterOperator[] | undefined) ??
      (shape.list
        ? (["INCLUDES"] as FilterOperator[])
        : (["EQ", "IN"] as FilterOperator[])
      ).filter((op) => allowed.includes(op));
    const bad = requested.filter((op) => !allowed.includes(op));
    if (bad.length > 0) {
      at(
        `${type} does not support ${bad.join(", ")}` +
          (allowed.length > 0 ? ` (allowed: ${allowed.join(", ")})` : ""),
      );
    }
    filters = new Set(requested.filter((op) => allowed.includes(op)));
  }
  // A key is always addressable by value.
  if (key) {
    filters.add("EQ");
    filters.add("IN");
  }
  if (isPrivate && (filterable || sortable)) {
    at("@private fields cannot be @filterable or @sortable");
  }
  if (sortable && (shape.list || UNSORTABLE.has(type))) {
    at(`${shape.list ? "lists" : type} cannot be @sortable`);
  }
  if (
    groupBy &&
    (shape.list ||
      type === "Point" ||
      type === "CartesianPoint" ||
      isPrivate ||
      !opts.allowKey)
  ) {
    at("@groupBy needs a readable, non-list, non-Point field of a @node type");
  }

  const property = (alias?.["property"] as string | undefined) ?? f.name;
  if (property.length === 0) at("@alias(property:) is empty");

  const indexes: IndexKind[] = [];
  if (index) {
    const kind = index["kind"] as IndexKind;
    if (kind === "TEXT" && type !== "String" && type !== "ID") {
      at("a TEXT index needs a String or ID field");
    } else if (
      kind === "POINT" &&
      type !== "Point" &&
      type !== "CartesianPoint"
    ) {
      at("a POINT index needs a Point field");
    } else if (
      kind === "RANGE" &&
      (type === "Point" || type === "CartesianPoint")
    ) {
      at("use a POINT index for Point fields");
    } else if (kind === "RANGE" && RANGE_UNINDEXABLE.has(type)) {
      at(`${type} values have no order a RANGE index can use`);
    } else {
      indexes.push(kind);
    }
  }

  // A computed field (@timestamp, @populatedBy) is never client-settable,
  // unless `@settable(onCreate: true)` / `onUpdate: true` says so
  // explicitly and a field-level rule decides who may supply it: then the
  // input offers it, and the computation fills it when it is omitted.
  const computed = timestamp !== undefined || !!populatedArgs;
  const supplied = {
    create: computed && explicitlyTrue(f, "settable", "onCreate"),
    update: computed && !key && explicitlyTrue(f, "settable", "onUpdate"),
  };
  if ((supplied.create || supplied.update) && !opts.allowKey) {
    at(
      "a @timestamp or @populatedBy relationship property cannot be made @settable",
    );
    supplied.create = supplied.update = false;
  }
  for (const [op, arg] of [
    ["CREATE", "onCreate"],
    ["UPDATE", "onUpdate"],
  ] as const) {
    if (!supplied[op === "CREATE" ? "create" : "update"]) continue;
    if (!fieldAuthorization?.validate.some((r) => r.operations.has(op))) {
      at(
        `@settable(${arg}: true) on a @timestamp or @populatedBy field needs a field-level @authorization(validate:) rule for ${op}: it decides who may supply the value (the schema's bypass is no such rule)`,
      );
    }
  }

  const field: ScalarField = {
    kind: "scalar",
    name: f.name,
    property,
    type,
    enumName,
    customScalar,
    list: shape.list,
    required: shape.required,
    key,
    unique,
    private: isPrivate,
    relayId,
    filters,
    sortable: sortable && !isPrivate,
    groupBy,
    indexes,
    generate,
    ...(keyScope ? { keyScope } : {}),
    defaultValue,
    timestamp,
    readonly:
      readonlyFlag ||
      isPrivate ||
      (computed && !supplied.create && !supplied.update),
    settableOn: {
      create:
        !(readonlyFlag || isPrivate) &&
        (computed
          ? supplied.create
          : ((settableArgs?.["onCreate"] as boolean | undefined) ?? true)),
      update:
        !key &&
        !(readonlyFlag || isPrivate) &&
        (computed
          ? supplied.update
          : ((settableArgs?.["onUpdate"] as boolean | undefined) ?? true)),
    },
    selectableOn: {
      read:
        !isPrivate &&
        ((selectableArgs?.["onRead"] as boolean | undefined) ?? true),
      aggregate:
        (selectableArgs?.["onAggregate"] as boolean | undefined) ?? true,
    },
    populatedBy: populatedArgs
      ? {
          callback: populatedArgs["callback"] as string,
          operations: new Set(
            populatedArgs["operations"] as Array<"CREATE" | "UPDATE">,
          ),
        }
      : undefined,
    vector,
    authentication: authentication
      ? new Set(authentication["operations"] as AuthOperation[])
      : undefined,
    authenticationJwt: authentication?.["jwt"] as
      | AuthorizationWhere
      | undefined,
    authorization: fieldAuthorization,
    description: f.description ?? undefined,
  };
  if (populatedArgs && (timestamp || defaultArgs)) {
    at("@populatedBy cannot be combined with @timestamp or @default");
  }
  if (
    !isPrivate &&
    !field.selectableOn.read &&
    (field.filters.size > 0 || field.sortable)
  ) {
    at(
      "a field with @selectable(onRead: false) cannot be @filterable or @sortable",
    );
  }
  if (vectorQuery) vectorQueryNames.set(field, vectorQuery);
  return field;
}

/** The directive on `f` spells out `arg: true` (a default does not count). */
function explicitlyTrue(
  f: GraphQLField<unknown, unknown>,
  directiveName: string,
  arg: string,
): boolean {
  const dir = f.astNode?.directives?.find(
    (x) => x.name.value === directiveName,
  );
  const value = dir?.arguments?.find((a) => a.name.value === arg)?.value;
  return value?.kind === Kind.BOOLEAN && value.value;
}

function keyArgs(
  d: (n: string) => GraphQLDirective,
  f: GraphQLField<unknown, unknown>,
  at: (message: string) => void,
): Record<string, unknown> | undefined {
  return directive(d("key"), f, at);
}

export function defaultMatches(
  type: ScalarType,
  list: boolean,
  value: unknown,
): boolean {
  if (list) {
    return (
      Array.isArray(value) && value.every((v) => defaultMatches(type, false, v))
    );
  }
  switch (type) {
    case "Int":
    case "BigInt":
      return Number.isInteger(value);
    case "Float":
      return typeof value === "number";
    case "Boolean":
      return typeof value === "boolean";
    case "Point":
    case "CartesianPoint":
      return false;
    default:
      return typeof value === "string";
  }
}
