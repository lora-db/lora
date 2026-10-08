// Value rules a field declares with @length, @pattern, @format, @range,
// @multipleOf and @size, and the one function that applies them. Pure:
// the model build uses it to check defaults, the mutation path to check
// inputs before the transaction and computed values inside it, so a rule
// has one implementation and one meaning.
//
// Semantics, each chosen on purpose (docs/design/graphql-input-validation.md):
// null and absent values pass (nullability is the type's business); a
// string's length is its Unicode code points, as in JSON Schema and in
// LoraDB's size(); a pattern must match the whole value; on a list field
// @size is about the list and every other rule about each item.

/** The rules of one field. Arguments map onto JSON Schema keywords. */
export interface FieldConstraints {
  length?: { min?: number; max?: number; message?: string };
  pattern?: { source: string; regex: RegExp; message?: string };
  format?: { name: string; message?: string };
  range?: {
    min?: number;
    max?: number;
    exclusiveMin?: number;
    exclusiveMax?: number;
    message?: string;
  };
  multipleOf?: { value: number; message?: string };
  size?: { min?: number; max?: number; unique: boolean; message?: string };
  /** `@normalize`: applied before the rules, and to what is stored. */
  normalize?: {
    trim: boolean;
    unicode?: "NFC" | "NFD" | "NFKC" | "NFKD";
    case?: "LOWER" | "UPPER";
    emptyToNull: boolean;
  };
}

/** One rule a value breaks. `rule` is the JSON Schema keyword. */
export interface RuleFailure {
  rule: string;
  /** The rule's own numbers and names, for clients that localise. */
  params: Record<string, unknown>;
  message: string;
  /** The item of a list field the failure is about. */
  index?: number;
}

interface Format {
  test: (value: string) => boolean;
  message: string;
}

/** Longest input any format looks at: longer never matches. */
const FORMAT_MAX_LENGTH = 2048;

// Every expression here is linear: no nested unbounded repetition, and
// inner repetitions are bounded.
const LABEL = "[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?";
// The WHATWG HTML rule for input[type=email].
const EMAIL = new RegExp(
  `^[a-zA-Z0-9.!#$%&'*+/=?^_\`{|}~-]{1,64}@${LABEL}(?:\\.${LABEL})*$`,
);
const EMAIL_STRICT = new RegExp(
  `^[a-zA-Z0-9.!#$%&'*+/=?^_\`{|}~-]{1,64}@(?:${LABEL}\\.)+[a-zA-Z]{2,63}$`,
);
const HOSTNAME = new RegExp(`^${LABEL}(?:\\.${LABEL})*$`);
const IPV4 =
  /^(?:(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)\.){3}(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)$/;
const HEX_GROUP = /^[0-9a-fA-F]{1,4}$/;

/** RFC 4291 text form, with an optional embedded IPv4 tail; no zone id. */
function isIpv6(value: string): boolean {
  const halves = value.split("::");
  if (halves.length > 2) return false;
  const groups = (part: string): string[] | undefined => {
    if (part === "") return [];
    const out = part.split(":");
    return out.every((g) => g !== "") ? out : undefined;
  };
  const head = groups(halves[0]!);
  const tail = halves.length === 2 ? groups(halves[1]!) : [];
  if (!head || !tail) return false;
  const all = [...head, ...tail];
  let count = all.length;
  const last = all[all.length - 1];
  if (last !== undefined && last.includes(".")) {
    if (!IPV4.test(last)) return false;
    all.pop();
    count += 1; // an IPv4 tail stands for two groups
  }
  if (!all.every((g) => HEX_GROUP.test(g))) return false;
  return halves.length === 2 ? count < 8 : count === 8;
}

function isHttpUrl(value: string): boolean {
  try {
    const { protocol } = new URL(value);
    return protocol === "http:" || protocol === "https:";
  } catch {
    return false;
  }
}

/**
 * The formats `@format(as:)` knows. Named after JSON Schema's where the
 * meaning matches, and differently where it does not (`url` is the WHATWG
 * parser with an http/https allow-list, not RFC 3986; `e164` is a shape,
 * not proof a number exists).
 */
export const FORMATS: Readonly<Record<string, Format>> = Object.freeze({
  email: {
    test: (v) => v.length <= 254 && EMAIL.test(v),
    message: "must be an email address",
  },
  "email-strict": {
    test: (v) => v.length <= 254 && EMAIL_STRICT.test(v),
    message: "must be an email address with a top-level domain",
  },
  url: { test: isHttpUrl, message: "must be an http or https URL" },
  uuid: {
    test: (v) =>
      /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/.test(
        v,
      ),
    message: "must be a UUID",
  },
  hostname: {
    test: (v) => v.length <= 253 && HOSTNAME.test(v),
    message: "must be a host name",
  },
  ipv4: { test: (v) => IPV4.test(v), message: "must be an IPv4 address" },
  ipv6: { test: isIpv6, message: "must be an IPv6 address" },
  slug: {
    test: (v) => /^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(v),
    message: "must be lower-case letters and digits separated by hyphens",
  },
  "hex-color": {
    test: (v) =>
      /^#(?:[0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/.test(v),
    message: "must be a hex colour such as #1b6dff",
  },
  ulid: {
    test: (v) => /^[0-7][0-9A-HJKMNP-TV-Za-hjkmnp-tv-z]{25}$/.test(v),
    message: "must be a ULID",
  },
  e164: {
    test: (v) => /^\+[1-9]\d{6,14}$/.test(v),
    message:
      "must be a phone number in international form, such as +31201234567",
  },
});

function normalizeText(
  n: NonNullable<FieldConstraints["normalize"]>,
  value: string,
): string {
  let out = n.trim ? value.trim() : value;
  if (n.unicode) out = out.normalize(n.unicode);
  // Locale-independent on purpose: a stored value must not depend on
  // where the server runs.
  if (n.case === "LOWER") out = out.toLowerCase();
  else if (n.case === "UPPER") out = out.toUpperCase();
  return out;
}

/**
 * `value` as `@normalize` stores it: trimmed, normalised and case-mapped,
 * in that order, and null for an empty string under `emptyToNull`. Items
 * of a list are normalised one by one. Anything but a string is returned
 * as it is.
 */
export function normalizeValue(c: FieldConstraints, value: unknown): unknown {
  const n = c.normalize;
  if (!n) return value;
  if (Array.isArray(value)) {
    return value.map((item) =>
      typeof item === "string" ? normalizeText(n, item) : item,
    );
  }
  if (typeof value !== "string") return value;
  const out = normalizeText(n, value);
  return n.emptyToNull && out === "" ? null : out;
}

/** Unicode code points in `s`. */
export function codePoints(s: string): number {
  let n = 0;
  for (const _ of s) n++;
  return n;
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** Whether `value / divisor` is an integer, within float tolerance. */
function isMultiple(value: number, divisor: number): boolean {
  if (Number.isInteger(value) && Number.isInteger(divisor)) {
    return value % divisor === 0;
  }
  // `0.3 % 0.1` is not 0 in IEEE arithmetic: compare the quotient with
  // its rounding instead, scaled to the magnitude of the quotient.
  const quotient = value / divisor;
  return (
    Math.abs(quotient - Math.round(quotient)) <=
    1e-9 * Math.max(1, Math.abs(quotient))
  );
}

function checkScalar(c: FieldConstraints, value: unknown): RuleFailure[] {
  const out: RuleFailure[] = [];
  if (typeof value === "string") {
    if (c.length) {
      const { min, max, message } = c.length;
      // A string has at most as many code points as UTF-16 units, so the
      // count is only needed when the cheap length does not settle it.
      if (max !== undefined && value.length > max && codePoints(value) > max) {
        out.push({
          rule: "maxLength",
          params: { limit: max },
          message: message ?? `must be at most ${plural(max, "character")}`,
        });
      }
      if (
        min !== undefined &&
        (value.length < min || codePoints(value) < min)
      ) {
        out.push({
          rule: "minLength",
          params: { limit: min },
          message: message ?? `must be at least ${plural(min, "character")}`,
        });
      }
    }
    // A value over its length bound is not handed to an expression: the
    // bound is what keeps the match cheap.
    const tooLong = out.some((f) => f.rule === "maxLength");
    if (c.format && !tooLong) {
      const format = FORMATS[c.format.name]!;
      if (value.length > FORMAT_MAX_LENGTH || !format.test(value)) {
        out.push({
          rule: "format",
          params: { format: c.format.name },
          message: c.format.message ?? format.message,
        });
      }
    }
    if (c.pattern && !tooLong && !c.pattern.regex.test(value)) {
      out.push({
        rule: "pattern",
        params: { pattern: c.pattern.source },
        message: c.pattern.message ?? "does not match the required pattern",
      });
    }
    return out;
  }
  if (typeof value === "number" || typeof value === "bigint") {
    // Bounds are floats: a bigint beyond the safe range is compared
    // approximately, which the reference says.
    const n = Number(value);
    if (!Number.isFinite(n)) {
      if (c.range || c.multipleOf) {
        out.push({
          rule: "finite",
          params: {},
          message: "must be a finite number",
        });
      }
      return out;
    }
    if (c.range) {
      const { min, max, exclusiveMin, exclusiveMax, message } = c.range;
      const fail = (
        rule: string,
        limit: number,
        comparison: string,
        text: string,
      ) =>
        out.push({
          rule,
          params: { limit, comparison },
          message: message ?? text,
        });
      if (min !== undefined && n < min) {
        fail("minimum", min, ">=", `must be at least ${min}`);
      }
      if (exclusiveMin !== undefined && n <= exclusiveMin) {
        fail(
          "exclusiveMinimum",
          exclusiveMin,
          ">",
          `must be greater than ${exclusiveMin}`,
        );
      }
      if (max !== undefined && n > max) {
        fail("maximum", max, "<=", `must be at most ${max}`);
      }
      if (exclusiveMax !== undefined && n >= exclusiveMax) {
        fail(
          "exclusiveMaximum",
          exclusiveMax,
          "<",
          `must be less than ${exclusiveMax}`,
        );
      }
    }
    if (c.multipleOf && !isMultiple(n, c.multipleOf.value)) {
      out.push({
        rule: "multipleOf",
        params: { multipleOf: c.multipleOf.value },
        message:
          c.multipleOf.message ?? `must be a multiple of ${c.multipleOf.value}`,
      });
    }
  }
  return out;
}

/**
 * The rules `value` breaks, in declaration order; empty when it is valid.
 * A list is checked as a list (`@size`) and then item by item.
 */
export function checkValue(c: FieldConstraints, value: unknown): RuleFailure[] {
  if (value === null || value === undefined) return [];
  if (!Array.isArray(value)) return checkScalar(c, value);
  const out: RuleFailure[] = [];
  if (c.size) {
    const { min, max, unique, message } = c.size;
    if (max !== undefined && value.length > max) {
      out.push({
        rule: "maxItems",
        params: { limit: max },
        message: message ?? `must have at most ${plural(max, "item")}`,
      });
    }
    if (min !== undefined && value.length < min) {
      out.push({
        rule: "minItems",
        params: { limit: min },
        message: message ?? `must have at least ${plural(min, "item")}`,
      });
    }
    // Scalars only reach here, so a Set of their text is exact. The list
    // is not walked when it is already over its size.
    if (
      unique &&
      (max === undefined || value.length <= max) &&
      new Set(value.map((v) => `${typeof v}:${String(v)}`)).size !==
        value.length
    ) {
      out.push({
        rule: "uniqueItems",
        params: {},
        message: message ?? "must not contain the same item twice",
      });
    }
  }
  for (const [index, item] of value.entries()) {
    for (const failure of checkScalar(c, item)) out.push({ ...failure, index });
  }
  return out;
}

/** Budget of one probe match; a linear match takes microseconds. */
const PROBE_BUDGET_MS = 25;
const now = () => globalThis.performance.now();

/**
 * Why `regex` is not safe to run on client input of up to `maxLength`
 * characters, or undefined. JavaScript's engine backtracks, so a pattern
 * such as `(a+)+` takes seconds on thirty characters. This runs the
 * compiled pattern on inputs built to make it backtrack (a character it
 * accepts, repeated, then one it rejects) and refuses a pattern that
 * overruns the budget. A probe, not a proof: it finds the repetition
 * mistakes people make, and `@length(max:)` bounds everything else.
 */
export function probePattern(
  regex: RegExp,
  source: string,
  maxLength: number,
): string | undefined {
  // Characters the pattern mentions, and a few every pattern meets.
  const seeds = new Set<string>(["a", "A", "0", " ", "-", ".", "@", "_"]);
  for (const ch of source.replace(/\\./g, "")) {
    if (/[\p{L}\p{N}]/u.test(ch)) seeds.add(ch);
  }
  const candidates = [...seeds].slice(0, 16);
  const pairs = candidates
    .slice(0, 4)
    .flatMap((a) => candidates.slice(0, 4).map((b) => a + b))
    .filter((p) => p[0] !== p[1]);
  const lengths = [
    ...[12, 16, 20, 24, 28, 32].filter((n) => n < maxLength),
    Math.min(maxLength, 5000),
  ];
  const slow = (input: string): boolean => {
    // Best of three: one slow run may be the machine, three are the pattern.
    for (let attempt = 0; attempt < 3; attempt++) {
      const started = now();
      regex.test(input);
      if (now() - started < PROBE_BUDGET_MS) return false;
    }
    return true;
  };
  for (const n of lengths) {
    for (const unit of [...candidates, ...pairs]) {
      const body = unit.repeat(Math.ceil(n / unit.length)).slice(0, n);
      // A tail no character class of a sane pattern accepts forces the
      // engine to try every way of splitting the body.
      if (slow(body + "\u0000")) {
        return `takes too long on ${n} characters of input (catastrophic backtracking): simplify the repetition`;
      }
    }
  }
  return undefined;
}
