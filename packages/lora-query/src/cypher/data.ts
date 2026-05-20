/**
 * Cypher metadata used by autocomplete + hover tooltips.
 *
 * Function metadata is sourced from the engine via the WASM `builtins()`
 * endpoint so the editor stays in lockstep with the analyzer / executor
 * — no more hand-maintained signature lists drifting from the Rust
 * source of truth. We still keep a small editorial layer on top:
 *
 *   - `DOC_OVERRIDES` — hand-written `info` / `detail` strings that beat
 *     the synthesised defaults. Carries our existing wording for the
 *     common functions; any builtin not in the map gets a generic
 *     "Builtin function — arity N..M" blurb.
 *   - `NAMESPACE_INFO` — hand-written one-liners for each namespace
 *     (`math`, `string`, …). Used when the builtin registry actually
 *     ships members for the namespace; empty namespaces are dropped.
 *
 * Clauses, keywords, constants, and operators are entirely editorial and
 * still live as hand-written arrays below.
 */

import * as WasmModule from "../../wasm/lora_query_wasm.js";

export type CypherKind =
  | "clause"
  | "keyword"
  | "function"
  | "namespace"
  | "constant"
  | "operator";

export interface CypherToken {
  /** What the user sees + types. Keywords are upper-case by convention. */
  label: string;
  /** Drives the colour + icon in the autocomplete popup. */
  kind: CypherKind;
  /** Short signature shown next to the label. */
  detail?: string;
  /** Longer explanation shown in the popup body / hover tooltip. */
  info: string;
}

export const CYPHER_CLAUSES: CypherToken[] = [
  {
    label: "MATCH",
    kind: "clause",
    detail: "MATCH pattern [WHERE ...]",
    info: "Reads a pattern from the graph. Combine with WHERE to filter.",
  },
  {
    label: "OPTIONAL MATCH",
    kind: "clause",
    detail: "OPTIONAL MATCH pattern",
    info: "Like MATCH, but returns NULL for missing parts instead of dropping the row.",
  },
  {
    label: "WHERE",
    kind: "clause",
    detail: "WHERE expression",
    info: "Filters the working set with a boolean expression.",
  },
  {
    label: "RETURN",
    kind: "clause",
    detail: "RETURN expression [AS alias]",
    info: "Projects expressions out of the query. Supports DISTINCT, ORDER BY, SKIP, LIMIT.",
  },
  {
    label: "WITH",
    kind: "clause",
    detail: "WITH expression [AS alias]",
    info: "Pipes the current rows into the next part of the query. Use to chain aggregations.",
  },
  {
    label: "UNWIND",
    kind: "clause",
    detail: "UNWIND list AS variable",
    info: "Turns a list into one row per item.",
  },
  {
    label: "CREATE",
    kind: "clause",
    detail: "CREATE pattern",
    info: "Creates nodes and relationships.",
  },
  {
    label: "MERGE",
    kind: "clause",
    detail: "MERGE pattern [ON CREATE SET ...] [ON MATCH SET ...]",
    info: "Match if it exists, otherwise create it.",
  },
  {
    label: "DELETE",
    kind: "clause",
    detail: "DELETE expression",
    info: "Removes nodes / relationships. Prefix with DETACH to drop relationships first.",
  },
  {
    label: "DETACH DELETE",
    kind: "clause",
    detail: "DETACH DELETE expression",
    info: "Deletes a node together with all of its relationships.",
  },
  {
    label: "SET",
    kind: "clause",
    detail: "SET target = value",
    info: "Assigns a property or label.",
  },
  {
    label: "REMOVE",
    kind: "clause",
    detail: "REMOVE target",
    info: "Removes a property or label.",
  },
  {
    label: "CALL",
    kind: "clause",
    detail: "CALL procedure(...) [YIELD ...]",
    info: "Invokes a stored procedure.",
  },
  {
    label: "YIELD",
    kind: "clause",
    detail: "YIELD field [AS alias]",
    info: "Names the columns produced by a CALL.",
  },
  {
    label: "ORDER BY",
    kind: "clause",
    detail: "ORDER BY expression [ASC|DESC]",
    info: "Sorts the result set.",
  },
  { label: "LIMIT", kind: "clause", detail: "LIMIT n", info: "Caps the number of rows returned." },
  { label: "SKIP", kind: "clause", detail: "SKIP n", info: "Skips the first `n` rows." },
  {
    label: "UNION",
    kind: "clause",
    detail: "UNION",
    info: "Concatenates the results of two queries with duplicate removal.",
  },
  {
    label: "UNION ALL",
    kind: "clause",
    detail: "UNION ALL",
    info: "Concatenates the results of two queries, keeping duplicates.",
  },
];

export const CYPHER_KEYWORDS: CypherToken[] = [
  { label: "AS", kind: "keyword", info: "Aliases an expression in a projection." },
  { label: "DISTINCT", kind: "keyword", info: "Drops duplicate rows from RETURN / WITH." },
  { label: "ASC", kind: "keyword", info: "Ascending sort order." },
  { label: "DESC", kind: "keyword", info: "Descending sort order." },
  { label: "IN", kind: "operator", info: "Membership test against a list." },
  { label: "IS NULL", kind: "operator", info: "Tests for NULL." },
  { label: "IS NOT NULL", kind: "operator", info: "Tests for non-NULL." },
  { label: "AND", kind: "operator", info: "Boolean conjunction." },
  { label: "OR", kind: "operator", info: "Boolean disjunction." },
  { label: "XOR", kind: "operator", info: "Boolean exclusive-or." },
  { label: "NOT", kind: "operator", info: "Boolean negation." },
  { label: "CASE", kind: "keyword", detail: "CASE ... WHEN ... THEN ... END", info: "Conditional expression." },
  { label: "EXISTS", kind: "keyword", detail: "EXISTS { pattern }", info: "True if the inner pattern matches at least once." },
];

export const CYPHER_CONSTANTS: CypherToken[] = [
  { label: "TRUE", kind: "constant", info: "Boolean true." },
  { label: "FALSE", kind: "constant", info: "Boolean false." },
  { label: "NULL", kind: "constant", info: "Absence of value." },
];

// ─────────────────────────────────────────────────────────────────────
// Editorial overrides for builtin functions
// ─────────────────────────────────────────────────────────────────────

interface DocOverride {
  info: string;
  detail?: string;
}

/**
 * Hand-written `info` / `detail` strings keyed by canonical builtin
 * name (e.g. `"math.abs"`, `"count"`). Salvaged from the previous
 * hand-maintained arrays — anything not in here gets a synthesised
 * placeholder derived from the signature returned by the registry.
 */
const DOC_OVERRIDES: Record<string, DocOverride> = {
  // Aggregates (top-level).
  count: { detail: "count(expr | *)", info: "Aggregate — number of rows. `count(*)` counts every row." },
  collect: { detail: "collect(expr)", info: "Aggregate — gather rows into a list." },
  sum: { detail: "sum(expr)", info: "Aggregate — numeric sum." },
  avg: { detail: "avg(expr)", info: "Aggregate — numeric average." },
  min: { detail: "min(expr)", info: "Aggregate — smallest value." },
  max: { detail: "max(expr)", info: "Aggregate — largest value." },
  stdev: { detail: "stdev(expr)", info: "Aggregate — sample standard deviation." },
  stdevp: { detail: "stdevp(expr)", info: "Aggregate — population standard deviation." },
  percentileCont: { detail: "percentileCont(expr, p)", info: "Aggregate — continuous percentile (linear interpolation)." },
  percentileDisc: { detail: "percentileDisc(expr, p)", info: "Aggregate — discrete percentile (nearest data point)." },
  // General scalar helpers (top-level).
  size: { detail: "size(list)", info: "Length of a list, string, or map." },
  length: { detail: "length(path)", info: "Number of relationships in a path." },
  keys: { detail: "keys(map)", info: "Keys of a map or properties of a node/relationship." },
  properties: { detail: "properties(node|rel)", info: "Properties of a node or relationship as a map." },
  coalesce: { detail: "coalesce(x, y, …)", info: "First non-null argument." },
  reverse: { detail: "reverse(list|string)", info: "Reverses a list or string." },
  is_null: { detail: "is_null(x)", info: "True when `x` is NULL." },
  id: { detail: "id(node|rel)", info: "Internal identifier of a node or relationship." },
  // Pattern helpers (top-level).
  nodes: { detail: "nodes(path)", info: "All nodes in a path." },
  relationships: { detail: "relationships(path)", info: "All relationships in a path." },
  type: { detail: "type(rel)", info: "Type of a relationship as a string." },
  labels: { detail: "labels(node)", info: "Labels of a node as a list of strings." },
  // List builders (top-level).
  range: { detail: "range(start, end[, step])", info: "Build an integer range." },
  head: { detail: "head(list)", info: "First element of a list." },
  tail: { detail: "tail(list)", info: "All elements after the first." },
  last: { detail: "last(list)", info: "Last element of a list." },
  timestamp: { detail: "timestamp()", info: "Current epoch milliseconds (UTC)." },
  // math.*
  "math.abs": { detail: "math.abs(x)", info: "Absolute value." },
  "math.sqrt": { detail: "math.sqrt(x)", info: "Square root." },
  "math.floor": { detail: "math.floor(x)", info: "Largest integer ≤ x." },
  "math.ceil": { detail: "math.ceil(x)", info: "Smallest integer ≥ x." },
  "math.round": { detail: "math.round(x)", info: "Banker's rounding." },
  "math.min": { detail: "math.min(a, b)", info: "Smaller of two values." },
  "math.max": { detail: "math.max(a, b)", info: "Larger of two values." },
  "math.clamp": { detail: "math.clamp(x, lo, hi)", info: "Clamp `x` to `[lo, hi]`." },
  "math.pow": { detail: "math.pow(base, exp)", info: "Exponentiation." },
  "math.log": { detail: "math.log(x)", info: "Natural logarithm." },
  "math.sin": { detail: "math.sin(x)", info: "Sine (radians)." },
  "math.cos": { detail: "math.cos(x)", info: "Cosine (radians)." },
  "math.tan": { detail: "math.tan(x)", info: "Tangent (radians)." },
  // string.*
  "string.upper": { detail: "string.upper(s)", info: "Upper-case." },
  "string.lower": { detail: "string.lower(s)", info: "Lower-case." },
  "string.length": { detail: "string.length(s)", info: "Character length." },
  "string.concat": { detail: "string.concat(a, b)", info: "Concatenate strings." },
  "string.contains": { detail: "string.contains(s, sub)", info: "Substring test." },
  "string.startsWith": { detail: "string.startsWith(s, prefix)", info: "Prefix test." },
  "string.endsWith": { detail: "string.endsWith(s, suffix)", info: "Suffix test." },
  "string.split": { detail: "string.split(s, sep)", info: "Split by separator." },
  "string.trim": { detail: "string.trim(s)", info: "Trim surrounding whitespace." },
  "string.replace": { detail: "string.replace(s, from, to)", info: "Substring replace." },
  "string.capitalize": { detail: "string.capitalize(s)", info: "Capitalise first letter." },
  "string.camel": { detail: "string.camel(s)", info: "Convert to camelCase." },
  "string.count": { detail: "string.count(s, sub)", info: "Occurrences of `sub`." },
  // list.*
  "list.sum": { detail: "list.sum(xs)", info: "Sum of numeric items." },
  "list.avg": { detail: "list.avg(xs)", info: "Average of numeric items." },
  "list.size": { detail: "list.size(xs)", info: "Number of items." },
  "list.append": { detail: "list.append(xs, item)", info: "Append an item." },
  "list.first": { detail: "list.first(xs)", info: "First item." },
  "list.last": { detail: "list.last(xs)", info: "Last item." },
  "list.contains": { detail: "list.contains(xs, item)", info: "Membership test." },
  "list.reverse": { detail: "list.reverse(xs)", info: "Reverse." },
  // map.*
  "map.keys": { detail: "map.keys(m)", info: "Keys of the map." },
  "map.size": { detail: "map.size(m)", info: "Number of entries." },
  // temporal.*
  "temporal.now": { detail: "temporal.now()", info: "Current timestamp." },
  "temporal.date": { detail: "temporal.date(...)", info: "Construct a date." },
  "temporal.datetime": { detail: "temporal.datetime(...)", info: "Construct a datetime." },
};

/**
 * Hand-written one-liners for each namespace. Only namespaces that
 * actually have members in the builtin registry are surfaced in
 * `CYPHER_NAMESPACES`; everything else falls back to a generic blurb.
 */
const NAMESPACE_INFO: Record<string, string> = {
  math: "Numeric helpers — `math.abs`, `math.sqrt`, `math.floor`, ...",
  string: "Text helpers — `string.upper`, `string.lower`, `string.length`, `string.concat`, ...",
  list: "List helpers — `list.sum`, `list.avg`, `list.append`, `list.size`, ...",
  map: "Map helpers — `map.keys`, `map.size`, ...",
  bytes: "Byte-string helpers.",
  bits: "Bit-level helpers.",
  json: "JSON parse / stringify.",
  uuid: "UUID generation + parsing.",
  cast: "Explicit type casts.",
  type: "Type predicates and reflection.",
  temporal: "Date / time helpers.",
  geo: "Geospatial helpers.",
  vector: "Vector / embedding helpers.",
  crypto: "Hashing + crypto helpers.",
};

// ─────────────────────────────────────────────────────────────────────
// WASM bridge — load the builtin registry at module init
// ─────────────────────────────────────────────────────────────────────

interface RawBuiltinFunction {
  name: string;
  minArgs: number;
  maxArgs: number | null;
  isAggregate: boolean;
  acceptsEnumAt: number[];
  acceptsTypeAt: number[];
}

interface RawBuiltinAlias {
  alias: string;
  canonical: string;
}

interface RawBuiltinsRegistry {
  functions: RawBuiltinFunction[];
  aliases: RawBuiltinAlias[];
}

/**
 * Pull the builtin registry from the WASM module. If the current
 * artifact doesn't expose `builtins()` yet (i.e. we're running with
 * the pre-rebuild shim), fall back to a registry synthesised from
 * `DOC_OVERRIDES` so existing tests + autocomplete coverage continue
 * to work. Once `yarn build:wasm` regenerates the shim, this call
 * starts returning the full 190+ entries automatically and the
 * fallback becomes dead code.
 */
function loadBuiltinsRaw(): RawBuiltinsRegistry {
  // TODO: enable after yarn build:wasm — `builtins` will be present
  // on the WASM module and the fallback path below becomes dead code.
  const mod = WasmModule as unknown as {
    builtins?: () => RawBuiltinsRegistry;
  };
  if (typeof mod.builtins === "function") {
    try {
      const raw = mod.builtins();
      if (raw && Array.isArray(raw.functions) && raw.functions.length > 0) {
        return raw;
      }
    } catch {
      // Fall through to the editorial fallback.
    }
  }
  return fallbackFromDocOverrides();
}

/**
 * Synthesise a minimal registry from the keys of `DOC_OVERRIDES` so
 * the editor stays usable while waiting for a WASM rebuild. Arity is
 * unknown here, so we default to variadic (`min=0, max=null`) — the
 * autocomplete + hover surfaces only need the name, and the synthesised
 * detail string is overridden by the editorial entry anyway.
 */
function fallbackFromDocOverrides(): RawBuiltinsRegistry {
  const AGGREGATE_NAMES = new Set([
    "count",
    "collect",
    "sum",
    "avg",
    "min",
    "max",
    "stdev",
    "stdevp",
    "percentileCont",
    "percentileDisc",
  ]);
  const functions: RawBuiltinFunction[] = Object.keys(DOC_OVERRIDES).map((name) => ({
    name,
    minArgs: 0,
    maxArgs: null,
    isAggregate: AGGREGATE_NAMES.has(name),
    acceptsEnumAt: [],
    acceptsTypeAt: [],
  }));
  return { functions, aliases: [] };
}

/** Render an arity range as a human placeholder, e.g. `(x, y)` or `(...)`. */
function synthDetailArgs(min: number, max: number | null): string {
  if (min === 0 && max === 0) return "()";
  if (max === null) {
    // Variadic — use "..." after `min` required placeholders.
    if (min === 0) return "(...)";
    const required = Array.from({ length: min }, (_, i) => `arg${i + 1}`).join(", ");
    return `(${required}, ...)`;
  }
  const required = Array.from({ length: min }, (_, i) => `arg${i + 1}`);
  const optional = Array.from({ length: max - min }, (_, i) => `[arg${min + i + 1}]`);
  return `(${[...required, ...optional].join(", ")})`;
}

function synthInfo(min: number, max: number | null, isAggregate: boolean): string {
  const arity = max === null ? `arity ${min}+` : min === max ? `arity ${min}` : `arity ${min}..${max}`;
  const lead = isAggregate ? "Aggregate" : "Builtin function";
  return `${lead} — ${arity}.`;
}

function buildToken(fn: RawBuiltinFunction): CypherToken {
  const override = DOC_OVERRIDES[fn.name];
  // For namespaced names, the autocomplete popup shows just the
  // member portion as the label (matches the legacy shape); the full
  // canonical name is only used for lookup.
  const dotIdx = fn.name.indexOf(".");
  const label = dotIdx >= 0 ? fn.name.slice(dotIdx + 1) : fn.name;
  const detail = override?.detail ?? `${fn.name}${synthDetailArgs(fn.minArgs, fn.maxArgs)}`;
  const info = override?.info ?? synthInfo(fn.minArgs, fn.maxArgs, fn.isAggregate);
  return { label, kind: "function", detail, info };
}

// ─────────────────────────────────────────────────────────────────────
// Derived, exported tables
// ─────────────────────────────────────────────────────────────────────

const RAW_REGISTRY: RawBuiltinsRegistry = loadBuiltinsRaw();

const TOP_LEVEL_FUNCTIONS: CypherToken[] = [];
const NAMESPACE_MEMBERS_BUILDER: Record<string, CypherToken[]> = {};

for (const fn of RAW_REGISTRY.functions) {
  if (!fn.name.includes(".")) {
    TOP_LEVEL_FUNCTIONS.push(buildToken(fn));
    continue;
  }
  const ns = fn.name.slice(0, fn.name.indexOf("."));
  if (!NAMESPACE_MEMBERS_BUILDER[ns]) NAMESPACE_MEMBERS_BUILDER[ns] = [];
  NAMESPACE_MEMBERS_BUILDER[ns]!.push(buildToken(fn));
}

// Sort each namespace alphabetically so completion ordering is stable
// across WASM rebuilds (the registry walks a HashMap in Rust).
for (const ns of Object.keys(NAMESPACE_MEMBERS_BUILDER)) {
  NAMESPACE_MEMBERS_BUILDER[ns]!.sort((a, b) => a.label.localeCompare(b.label));
}
TOP_LEVEL_FUNCTIONS.sort((a, b) => a.label.localeCompare(b.label));

export const CYPHER_TOP_LEVEL_FUNCTIONS: CypherToken[] = TOP_LEVEL_FUNCTIONS;

export const NAMESPACE_MEMBERS: Record<string, CypherToken[]> = NAMESPACE_MEMBERS_BUILDER;

/**
 * Only namespaces that actually have members in the registry are
 * surfaced — no empty `bytes` / `bits` / `json` entries with nothing
 * behind them. Sorted alphabetically for stable completion ordering.
 */
export const CYPHER_NAMESPACES: CypherToken[] = Object.keys(NAMESPACE_MEMBERS_BUILDER)
  .sort()
  .map((ns) => ({
    label: ns,
    kind: "namespace" as const,
    info: NAMESPACE_INFO[ns] ?? `${ns}.* helpers.`,
  }));

export interface CypherAlias {
  alias: string;
  canonical: string;
  kind: "alias";
}

/**
 * Compatibility aliases (e.g. `tolower → string.lower`,
 * `date → temporal.now`). Surfaced by the consumer as a low-boost
 * suggestion category so users typing legacy Neo4j-style names still
 * find the canonical builtin.
 */
export const CYPHER_ALIASES: CypherAlias[] = RAW_REGISTRY.aliases
  .map((a) => ({ alias: a.alias, canonical: a.canonical, kind: "alias" as const }))
  .sort((a, b) => a.alias.localeCompare(b.alias));

// ─────────────────────────────────────────────────────────────────────
// Lookup index
// ─────────────────────────────────────────────────────────────────────

const ALL_KEYS = new Map<string, CypherToken>();
function index(tokens: CypherToken[], keyLowercase = false) {
  for (const t of tokens) {
    ALL_KEYS.set(keyLowercase ? t.label.toLowerCase() : t.label.toUpperCase(), t);
  }
}
index(CYPHER_CLAUSES);
index(CYPHER_KEYWORDS);
index(CYPHER_CONSTANTS);
index(CYPHER_TOP_LEVEL_FUNCTIONS, true);
index(CYPHER_NAMESPACES, true);
for (const [ns, members] of Object.entries(NAMESPACE_MEMBERS)) {
  for (const m of members) {
    ALL_KEYS.set(`${ns}.${m.label}`.toLowerCase(), m);
  }
}

// Resolve aliases to their canonical entry. The lookup is best-effort —
// if the alias points at a name we don't have a token for (e.g. the
// WASM artifact predates a builtin), we fall through to `undefined`.
const ALIAS_INDEX = new Map<string, string>();
for (const a of CYPHER_ALIASES) {
  ALIAS_INDEX.set(a.alias.toLowerCase(), a.canonical);
}

/** Look up a token by its identifier (case-insensitive). Aliases are
 * resolved transparently — `findToken("tolower")` returns the token
 * for `string.lower`. */
export function findToken(name: string): CypherToken | undefined {
  const direct =
    ALL_KEYS.get(name.toUpperCase()) ?? ALL_KEYS.get(name.toLowerCase());
  if (direct) return direct;
  const canonical = ALIAS_INDEX.get(name.toLowerCase());
  if (!canonical) return undefined;
  return ALL_KEYS.get(canonical.toLowerCase());
}

/**
 * Look up a function token by canonical or alias name. Returns
 * `undefined` if neither match. Convenience wrapper around
 * [`findToken`] that filters to function-shaped results so callers
 * (e.g. signature hints) don't have to discriminate the kind.
 */
export function findFunction(name: string): CypherToken | undefined {
  const tok = findToken(name);
  return tok && tok.kind === "function" ? tok : undefined;
}
