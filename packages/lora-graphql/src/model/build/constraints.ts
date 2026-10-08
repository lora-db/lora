// @length, @pattern, @format, @range, @multipleOf and @size on a field:
// read, checked against the field's type, and compiled.

import type { GraphQLDirective, GraphQLScalarType } from "graphql";
import {
  checkValue,
  FORMATS,
  normalizeValue,
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
  f: { astNode?: unknown; extensionASTNodes?: unknown },
  at: (message: string) => void,
  field: {
    type: ScalarType;
    list: boolean;
    vector: boolean;
    timestamp: boolean;
    defaultValue: { value: unknown } | undefined;
    /** The field's custom scalar, whose rules it inherits. */
    scalar?: GraphQLScalarType | undefined;
    /** How a problem names the default: an argument's is not a directive. */
    defaultLabel?: string | undefined;
    /** Directives another reader handles (an argument's plain @size, @range). */
    skip?: ReadonlySet<string> | undefined;
  },
): FieldConstraints | undefined {
  // A rule on the field wins; one on the field's custom scalar applies to
  // every field of that type, which is how a constrained type is reused.
  const read = (name: string) =>
    field.skip?.has(name)
      ? undefined
      : (directive(d(name), f, at) ??
        (field.scalar
          ? directive(d(name), field.scalar, (message) =>
              at(`${message} (on scalar ${field.scalar!.name})`),
            )
          : undefined));
  const lengthArgs = read("length");
  const patternArgs = read("pattern");
  const formatArgs = read("format");
  const rangeArgs = read("range");
  const multipleArgs = read("multipleOf");
  const sizeArgs = field.skip?.has("size")
    ? undefined
    : directive(d("size"), f, at);
  const normalizeArgs = read("normalize");
  const used = (
    [
      ["length", lengthArgs],
      ["pattern", patternArgs],
      ["format", formatArgs],
      ["range", rangeArgs],
      ["multipleOf", multipleArgs],
      ["size", sizeArgs],
      ["normalize", normalizeArgs],
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

  if (normalizeArgs) {
    const trim = normalizeArgs["trim"] === true;
    const emptyToNull = normalizeArgs["emptyToNull"] === true;
    const unicode = normalizeArgs["unicode"] as
      | NonNullable<FieldConstraints["normalize"]>["unicode"]
      | null
      | undefined;
    const textCase = normalizeArgs["case"] as
      | "LOWER"
      | "UPPER"
      | null
      | undefined;
    if (!isText) at("@normalize applies to String and ID fields");
    else if (!trim && !emptyToNull && !unicode && !textCase) {
      at("@normalize needs trim, case, unicode or emptyToNull");
    } else if (emptyToNull && field.list) {
      at(
        "@normalize(emptyToNull:) applies to a single value, not to list items",
      );
    } else {
      out.normalize = {
        trim,
        emptyToNull,
        ...(unicode ? { unicode } : {}),
        ...(textCase ? { case: textCase } : {}),
      };
    }
  }

  if (Object.keys(out).length === 0) return undefined;
  if (field.defaultValue) {
    const value = field.defaultValue.value;
    const defaultLabel = field.defaultLabel ?? "@default(value:)";
    // A default is stored as written: it has to be in the stored form
    // already, and pass the rules a client's value would.
    if (JSON.stringify(normalizeValue(out, value)) !== JSON.stringify(value)) {
      at(`${defaultLabel} is not in the form @normalize stores`);
    }
    const [broken] = checkValue(out, value);
    if (broken) at(`${defaultLabel} ${broken.message}`);
  }
  return out;
}
