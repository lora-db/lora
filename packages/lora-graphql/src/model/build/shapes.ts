// Small readers of GraphQL types shared by the model build: the built-in
// scalars, how a field's type unwraps (list, required, items), a custom
// scalar's storage type, and the names derived from a type's name.

import {
  isEnumType,
  isListType,
  isNonNullType,
  isScalarType,
  Kind,
  type GraphQLNamedType,
  type GraphQLInputType,
  type GraphQLOutputType,
  type GraphQLSchema,
} from "graphql";
import type { TypeShape, ScalarType } from "../types.js";
import { directiveNodes } from "./directive-args.js";

export const BUILTIN_SCALARS: Record<string, ScalarType> = {
  String: "String",
  ID: "ID",
  Int: "Int",
  Float: "Float",
  Boolean: "Boolean",
  BigInt: "BigInt",
  Date: "Date",
  Time: "Time",
  LocalTime: "LocalTime",
  DateTime: "DateTime",
  LocalDateTime: "LocalDateTime",
  Duration: "Duration",
  Point: "Point",
  CartesianPoint: "CartesianPoint",
};

export function unwrap(type: GraphQLOutputType): {
  required: boolean;
  list: boolean;
  itemRequired: boolean;
  depth: number;
} {
  let required = false;
  let t: GraphQLOutputType = type;
  if (isNonNullType(t)) {
    required = true;
    t = t.ofType;
  }
  let depth = 0;
  let itemRequired = false;
  while (isListType(t)) {
    depth++;
    t = t.ofType as GraphQLOutputType;
    if (isNonNullType(t)) {
      itemRequired = true;
      t = t.ofType;
    } else {
      itemRequired = false;
    }
  }
  return { required, list: depth > 0, itemRequired, depth };
}

export function isScalarLike(name: string, schema: GraphQLSchema): boolean {
  const t = schema.getType(name);
  return (
    name in BUILTIN_SCALARS ||
    (t !== undefined && (isEnumType(t) || storageOf(t) !== undefined))
  );
}

const STORAGE_TYPES: Record<string, ScalarType> = {
  STRING: "String",
  INT: "Int",
  FLOAT: "Float",
  BOOLEAN: "Boolean",
  DATETIME: "DateTime",
  DATE: "Date",
};

/** A custom scalar's storage type, from its `@storedAs(type:)`. */
export function storageOf(t: GraphQLNamedType): ScalarType | undefined {
  if (!isScalarType(t)) return undefined;
  const d = directiveNodes("storedAs", t)[0]?.node;
  const arg = d?.arguments?.find((a) => a.name.value === "type")?.value;
  return arg && arg.kind === Kind.ENUM ? STORAGE_TYPES[arg.value] : undefined;
}

export function lowerFirst(s: string): string {
  return s.charAt(0).toLowerCase() + s.slice(1);
}

export function defaultPlural(typeName: string): string {
  const base = lowerFirst(typeName);
  if (/[^aeiou]y$/i.test(base)) return base.slice(0, -1) + "ies";
  if (/(s|x|z|ch|sh)$/i.test(base)) return base + "es";
  return base + "s";
}

export function shapeOf(
  shape: ReturnType<typeof unwrap>,
): Omit<TypeShape, "named"> {
  return {
    list: shape.list,
    required: shape.required,
    itemRequired: shape.itemRequired,
  };
}

export function unwrapInput(type: GraphQLInputType): Omit<TypeShape, "named"> {
  let required = false;
  let t: GraphQLInputType = type;
  if (isNonNullType(t)) {
    required = true;
    t = t.ofType;
  }
  let list = false;
  let itemRequired = false;
  if (isListType(t)) {
    list = true;
    itemRequired = isNonNullType(t.ofType);
  }
  return { list, required, itemRequired };
}

export function upperFirst(s: string): string {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

export function snakeCase(s: string): string {
  return s
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .replace(/[^A-Za-z0-9]+/g, "_")
    .toLowerCase();
}
