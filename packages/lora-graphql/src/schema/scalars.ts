// Scalars for LoraDB value types. Temporal values travel as ISO-8601
// strings in GraphQL and as tagged `{ kind, iso }` objects to and from
// the engine; BigInt travels as a decimal string.

import { GraphQLError, GraphQLScalarType, Kind, type ValueNode } from "graphql";

type TemporalKind =
  | "date"
  | "time"
  | "localtime"
  | "datetime"
  | "localdatetime"
  | "duration";

function temporal(
  name: string,
  kind: TemporalKind,
  description: string,
): GraphQLScalarType {
  const parse = (value: unknown) => {
    if (typeof value !== "string" || value.length === 0) {
      throw new GraphQLError(`${name} must be an ISO-8601 string`);
    }
    return { kind, iso: value };
  };
  return new GraphQLScalarType({
    name,
    description,
    serialize(value) {
      if (typeof value === "string") return value;
      if (
        value !== null &&
        typeof value === "object" &&
        typeof (value as { iso?: unknown }).iso === "string"
      ) {
        return (value as { iso: string }).iso;
      }
      throw new GraphQLError(`${name} cannot represent ${String(value)}`);
    },
    parseValue: parse,
    parseLiteral(ast: ValueNode) {
      if (ast.kind !== Kind.STRING) {
        throw new GraphQLError(`${name} must be an ISO-8601 string`, {
          nodes: ast,
        });
      }
      return parse(ast.value);
    },
  });
}

export const GraphQLDate = temporal(
  "Date",
  "date",
  "A calendar date, e.g. `2026-06-21`.",
);
export const GraphQLTime = temporal(
  "Time",
  "time",
  "A time of day with offset, e.g. `21:00:00+02:00`.",
);
export const GraphQLLocalTime = temporal(
  "LocalTime",
  "localtime",
  "A time of day without offset, e.g. `21:00:00`.",
);
export const GraphQLDateTime = temporal(
  "DateTime",
  "datetime",
  "An instant with offset, e.g. `2026-06-21T21:00:00Z`.",
);
export const GraphQLLocalDateTime = temporal(
  "LocalDateTime",
  "localdatetime",
  "A date and time without offset, e.g. `2026-06-21T21:00:00`.",
);
export const GraphQLDuration = temporal(
  "Duration",
  "duration",
  "An ISO-8601 duration, e.g. `P1DT2H`.",
);

const MIN_SAFE = BigInt(Number.MIN_SAFE_INTEGER);
const MAX_SAFE = BigInt(Number.MAX_SAFE_INTEGER);

function parseBigInt(value: unknown): bigint {
  try {
    if (typeof value === "bigint") return value;
    if (typeof value === "number" && Number.isSafeInteger(value)) {
      return BigInt(value);
    }
    if (typeof value === "string" && /^-?\d+$/.test(value)) {
      return BigInt(value);
    }
  } catch {
    // fall through
  }
  throw new GraphQLError("BigInt must be an integer or a decimal string");
}

/** Pass safe integers as numbers; only true 64-bit values as bigint. */
function toParam(n: bigint): number | bigint {
  return n >= MIN_SAFE && n <= MAX_SAFE ? Number(n) : n;
}

export const GraphQLBigInt = new GraphQLScalarType({
  name: "BigInt",
  description: "A 64-bit integer, serialised as a decimal string.",
  serialize(value) {
    if (typeof value === "bigint") return value.toString();
    if (typeof value === "number" && Number.isInteger(value)) {
      return BigInt(value).toString();
    }
    throw new GraphQLError(`BigInt cannot represent ${String(value)}`);
  },
  parseValue: (value) => toParam(parseBigInt(value)),
  parseLiteral(ast) {
    if (ast.kind === Kind.INT || ast.kind === Kind.STRING) {
      return toParam(parseBigInt(ast.value));
    }
    throw new GraphQLError("BigInt must be an integer or a decimal string", {
      nodes: ast,
    });
  },
});

export const CUSTOM_SCALARS: Record<string, GraphQLScalarType> = {
  BigInt: GraphQLBigInt,
  Date: GraphQLDate,
  Time: GraphQLTime,
  LocalTime: GraphQLLocalTime,
  DateTime: GraphQLDateTime,
  LocalDateTime: GraphQLLocalDateTime,
  Duration: GraphQLDuration,
};
