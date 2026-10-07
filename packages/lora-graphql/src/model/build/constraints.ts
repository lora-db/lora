// @length, @pattern, @format, @range, @multipleOf and @size on a field:
// read, checked against the field's type, and compiled.

import type { GraphQLDirective, GraphQLField } from "graphql";
import {
  checkValue,
  FORMATS,
  probePattern,
  type FieldConstraints,
} from "../constraints.js";
import type { ScalarType } from "../types.js";
import { directive } from "./directive-args.js";

const TEXT = new Set<ScalarType>(["String", "ID"]);
const NUMBERS = new Set<ScalarType>(["Int", "Float", "BigInt"]);

const num = (args: Record<string, unknown>, key: string): number | undefined =>
  typeof args[key] === "number" ? (args[key] as number) : undefined;
const text = (args: Record<string, unknown>): { message?: string } =>
  typeof args["message"] === "string"
    ? { message: args["message"] as string }
    : {};

/** The value rules of field `f`, with every problem reported through `at`. */
export function readConstraints(
  d: (n: string) => GraphQLDirective,
  f: GraphQLField<unknown, unknown>,
  at: (message: string) => void,
  field: {
    type: ScalarType;
    list: boolean;
    vector: boolean;
    timestamp: boolean;
    defaultValue: { value: unknown } | undefined;
  },
): FieldConstraints | undefined {
  const lengthArgs = directive(d("length"), f, at);
  const patternArgs = directive(d("pattern"), f, at);
  const formatArgs = directive(d("format"), f, at);
  const rangeArgs = directive(d("range"), f, at);
  const multipleArgs = directive(d("multipleOf"), f, at);
  const sizeArgs = directive(d("size"), f, at);
  const used = (
    [
      ["length", lengthArgs],
      ["pattern", patternArgs],
      ["format", formatArgs],
      ["range", rangeArgs],
      ["multipleOf", multipleArgs],
      ["size", sizeArgs],
    ] as const
  ).filter(([, args]) => args !== undefined);
  if (used.length === 0) return undefined;
  const names = used.map(([n]) => `@${n}`).join(", ");
  if (field.timestamp) {
    at(`${names}: a @timestamp is set by the database, not validated`);
    return undefined;
  }
  if (field.vector) {
    at(`${names}: a @vector is bounded by its dimensions`);
    return undefined;
  }

  const out: FieldConstraints = {};
  const isText = TEXT.has(field.type);
  const isNumber = NUMBERS.has(field.type);

  if (lengthArgs) {
    const min = num(lengthArgs, "min");
    const max = num(lengthArgs, "max");
    if (!isText) at("@length applies to String and ID fields");
    else if (min === undefined && max === undefined) {
      at("@length needs min, max or both");
    } else if ((min ?? 0) < 0 || (max ?? 0) < 0) {
      at("@length bounds cannot be negative");
    } else if (min !== undefined && max !== undefined && min > max) {
      at("@length(min:) is greater than max");
    } else {
      out.length = {
        ...(min !== undefined ? { min } : {}),
        ...(max !== undefined ? { max } : {}),
        ...text(lengthArgs),
      };
    }
  }

  if (formatArgs) {
    const name = formatArgs["as"] as string;
    if (!isText) at("@format applies to String and ID fields");
    else if (!Object.hasOwn(FORMATS, name)) {
      at(
        `@format(as: "${name}") is not a known format (${Object.keys(FORMATS).join(", ")})`,
      );
    } else {
      out.format = { name, ...text(formatArgs) };
    }
  }

  if (patternArgs) {
    const source = patternArgs["regex"] as string;
    const max = out.length?.max;
    if (!isText) at("@pattern applies to String and ID fields");
    else if (max === undefined) {
      // The length bound is what keeps a match cheap on hostile input.
      at(
        "@pattern needs @length(max:) on the same field: an expression is only run on input of bounded length",
      );
    } else {
      let regex: RegExp | undefined;
      try {
        // The whole value must match, as with Cypher's `=~`.
        regex = new RegExp(`^(?:${source})$`, "u");
      } catch (err) {
        at(
          `@pattern(regex:) is not a valid expression: ${err instanceof Error ? err.message : String(err)}`,
        );
      }
      if (regex) {
        const unsafe = probePattern(regex, source, max);
        if (unsafe) at(`@pattern(regex:) ${unsafe}`);
        else out.pattern = { source, regex, ...text(patternArgs) };
      }
    }
  }

  if (rangeArgs) {
    const min = num(rangeArgs, "min");
    const max = num(rangeArgs, "max");
    const exclusiveMin = num(rangeArgs, "exclusiveMin");
    const exclusiveMax = num(rangeArgs, "exclusiveMax");
    const lower = min ?? exclusiveMin;
    const upper = max ?? exclusiveMax;
    if (!isNumber) at("@range applies to Int, Float and BigInt fields");
    else if (lower === undefined && upper === undefined) {
      at("@range needs a bound: min, max, exclusiveMin or exclusiveMax");
    } else if (min !== undefined && exclusiveMin !== undefined) {
      at("@range takes min or exclusiveMin, not both");
    } else if (max !== undefined && exclusiveMax !== undefined) {
      at("@range takes max or exclusiveMax, not both");
    } else if (
      lower !== undefined &&
      upper !== undefined &&
      (lower > upper ||
        (lower === upper &&
          (exclusiveMin !== undefined || exclusiveMax !== undefined)))
    ) {
      at("@range admits no value: its lower bound is above its upper bound");
    } else {
      out.range = {
        ...(min !== undefined ? { min } : {}),
        ...(max !== undefined ? { max } : {}),
        ...(exclusiveMin !== undefined ? { exclusiveMin } : {}),
        ...(exclusiveMax !== undefined ? { exclusiveMax } : {}),
        ...text(rangeArgs),
      };
    }
  }

  if (multipleArgs) {
    const value = multipleArgs["value"] as number;
    if (field.type !== "Int" && field.type !== "Float") {
      at("@multipleOf applies to Int and Float fields");
    } else if (!(value > 0)) {
      at("@multipleOf(value:) must be greater than 0");
    } else if (field.type === "Int" && !Number.isInteger(value)) {
      at("@multipleOf(value:) on an Int field must be an integer");
    } else {
      out.multipleOf = { value, ...text(multipleArgs) };
    }
  }

  if (sizeArgs) {
    const min = num(sizeArgs, "min");
    const max = num(sizeArgs, "max");
    const unique = sizeArgs["unique"] === true;
    if (!field.list) at("@size applies to list fields");
    else if (min === undefined && max === undefined && !unique) {
      at("@size needs min, max or unique");
    } else if ((min ?? 0) < 0 || (max ?? 0) < 0) {
      at("@size bounds cannot be negative");
    } else if (min !== undefined && max !== undefined && min > max) {
      at("@size(min:) is greater than max");
    } else {
      out.size = {
        ...(min !== undefined ? { min } : {}),
        ...(max !== undefined ? { max } : {}),
        unique,
        ...text(sizeArgs),
      };
    }
  }

  if (Object.keys(out).length === 0) return undefined;
  // A default the field's own rules refuse could never be stored by a client.
  if (field.defaultValue) {
    const [broken] = checkValue(out, field.defaultValue.value);
    if (broken) at(`@default(value:) ${broken.message}`);
  }
  return out;
}
