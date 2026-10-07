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

/**
 * A value this scalar refuses. Coded here, so that it is BAD_USER_INPUT
 * whichever way it arrived: as a variable, as a literal in the document
 * (which fails GraphQL validation, before any resolver), through
 * `execute()` or through another server on `getSchema()`.
 */
export function badInput(message: string, nodes?: ValueNode): GraphQLError {
  return new GraphQLError(message, {
    ...(nodes ? { nodes } : {}),
    extensions: { code: "BAD_USER_INPUT" },
  });
}

function temporal(
  name: string,
  kind: TemporalKind,
  description: string,
): GraphQLScalarType {
  const parse = (value: unknown) => {
    if (typeof value !== "string" || value.length === 0) {
      throw badInput(`${name} must be an ISO-8601 string`);
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
        throw badInput(`${name} must be an ISO-8601 string`, ast);
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
  throw badInput("BigInt must be an integer or a decimal string");
}

// The Node binding types a JavaScript number as an INTEGER only inside
// [-2^31, 2^32): a larger one, although an integer, is stored as a FLOAT.
// So only values in that range travel as numbers, and the rest as bigint,
// which the binding always takes as an INTEGER.
const NUMBER_MIN = -(2n ** 31n);
const NUMBER_END = 2n ** 32n;

function toParam(n: bigint): number | bigint {
  return n >= NUMBER_MIN && n < NUMBER_END ? Number(n) : n;
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
    throw badInput("BigInt must be an integer or a decimal string", ast);
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
