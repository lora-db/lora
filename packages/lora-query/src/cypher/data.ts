/**
 * Cypher metadata used by autocomplete + hover tooltips.
 *
 * The lists below intentionally describe the LoraDB dialect: namespaced
 * builtins under `math.*`, `string.*`, `list.*`, etc. Keeping the data
 * in one place lets the editor stay in lockstep with the engine without
 * duplicating signatures across the completion / hover surfaces.
 */

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

export const CYPHER_TOP_LEVEL_FUNCTIONS: CypherToken[] = [
  // Aggregates — callable directly at top level.
  { label: "count", kind: "function", detail: "count(expr | *)", info: "Aggregate — number of rows. `count(*)` counts every row." },
  { label: "collect", kind: "function", detail: "collect(expr)", info: "Aggregate — gather rows into a list." },
  { label: "sum", kind: "function", detail: "sum(expr)", info: "Aggregate — numeric sum." },
  { label: "avg", kind: "function", detail: "avg(expr)", info: "Aggregate — numeric average." },
  { label: "min", kind: "function", detail: "min(expr)", info: "Aggregate — smallest value." },
  { label: "max", kind: "function", detail: "max(expr)", info: "Aggregate — largest value." },
  { label: "stdev", kind: "function", detail: "stdev(expr)", info: "Aggregate — sample standard deviation." },
  { label: "stdevp", kind: "function", detail: "stdevp(expr)", info: "Aggregate — population standard deviation." },
  { label: "percentileCont", kind: "function", detail: "percentileCont(expr, p)", info: "Aggregate — continuous percentile (linear interpolation)." },
  { label: "percentileDisc", kind: "function", detail: "percentileDisc(expr, p)", info: "Aggregate — discrete percentile (nearest data point)." },
  // General scalar helpers.
  { label: "size", kind: "function", detail: "size(list)", info: "Length of a list, string, or map." },
  { label: "length", kind: "function", detail: "length(path)", info: "Number of relationships in a path." },
  { label: "keys", kind: "function", detail: "keys(map)", info: "Keys of a map or properties of a node/relationship." },
  { label: "properties", kind: "function", detail: "properties(node|rel)", info: "Properties of a node or relationship as a map." },
  { label: "coalesce", kind: "function", detail: "coalesce(x, y, …)", info: "First non-null argument." },
  { label: "reverse", kind: "function", detail: "reverse(list|string)", info: "Reverses a list or string." },
  { label: "is_null", kind: "function", detail: "is_null(x)", info: "True when `x` is NULL." },
  { label: "id", kind: "function", detail: "id(node|rel)", info: "Internal identifier of a node or relationship." },
  // Pattern helpers.
  { label: "nodes", kind: "function", detail: "nodes(path)", info: "All nodes in a path." },
  { label: "relationships", kind: "function", detail: "relationships(path)", info: "All relationships in a path." },
  { label: "type", kind: "function", detail: "type(rel)", info: "Type of a relationship as a string." },
  { label: "labels", kind: "function", detail: "labels(node)", info: "Labels of a node as a list of strings." },
  // List builders.
  { label: "range", kind: "function", detail: "range(start, end[, step])", info: "Build an integer range." },
  { label: "head", kind: "function", detail: "head(list)", info: "First element of a list." },
  { label: "tail", kind: "function", detail: "tail(list)", info: "All elements after the first." },
  { label: "last", kind: "function", detail: "last(list)", info: "Last element of a list." },
  { label: "timestamp", kind: "function", detail: "timestamp()", info: "Current epoch milliseconds (UTC)." },
];

export const CYPHER_NAMESPACES: CypherToken[] = [
  { label: "math", kind: "namespace", info: "Numeric helpers — `math.abs`, `math.sqrt`, `math.floor`, ..." },
  { label: "string", kind: "namespace", info: "Text helpers — `string.upper`, `string.lower`, `string.length`, `string.concat`, ..." },
  { label: "list", kind: "namespace", info: "List helpers — `list.sum`, `list.avg`, `list.append`, `list.size`, ..." },
  { label: "map", kind: "namespace", info: "Map helpers — `map.keys`, `map.size`, ..." },
  { label: "bytes", kind: "namespace", info: "Byte-string helpers." },
  { label: "bits", kind: "namespace", info: "Bit-level helpers." },
  { label: "json", kind: "namespace", info: "JSON parse / stringify." },
  { label: "uuid", kind: "namespace", info: "UUID generation + parsing." },
  { label: "cast", kind: "namespace", info: "Explicit type casts." },
  { label: "type", kind: "namespace", info: "Type predicates and reflection." },
  { label: "temporal", kind: "namespace", info: "Date / time helpers." },
  { label: "geo", kind: "namespace", info: "Geospatial helpers." },
  { label: "vector", kind: "namespace", info: "Vector / embedding helpers." },
  { label: "crypto", kind: "namespace", info: "Hashing + crypto helpers." },
];

export const NAMESPACE_MEMBERS: Record<string, CypherToken[]> = {
  math: [
    { label: "abs", kind: "function", detail: "math.abs(x)", info: "Absolute value." },
    { label: "sqrt", kind: "function", detail: "math.sqrt(x)", info: "Square root." },
    { label: "floor", kind: "function", detail: "math.floor(x)", info: "Largest integer ≤ x." },
    { label: "ceil", kind: "function", detail: "math.ceil(x)", info: "Smallest integer ≥ x." },
    { label: "round", kind: "function", detail: "math.round(x)", info: "Banker's rounding." },
    { label: "min", kind: "function", detail: "math.min(a, b)", info: "Smaller of two values." },
    { label: "max", kind: "function", detail: "math.max(a, b)", info: "Larger of two values." },
    { label: "clamp", kind: "function", detail: "math.clamp(x, lo, hi)", info: "Clamp `x` to `[lo, hi]`." },
    { label: "pow", kind: "function", detail: "math.pow(base, exp)", info: "Exponentiation." },
    { label: "log", kind: "function", detail: "math.log(x)", info: "Natural logarithm." },
    { label: "sin", kind: "function", detail: "math.sin(x)", info: "Sine (radians)." },
    { label: "cos", kind: "function", detail: "math.cos(x)", info: "Cosine (radians)." },
    { label: "tan", kind: "function", detail: "math.tan(x)", info: "Tangent (radians)." },
  ],
  string: [
    { label: "upper", kind: "function", detail: "string.upper(s)", info: "Upper-case." },
    { label: "lower", kind: "function", detail: "string.lower(s)", info: "Lower-case." },
    { label: "length", kind: "function", detail: "string.length(s)", info: "Character length." },
    { label: "concat", kind: "function", detail: "string.concat(a, b)", info: "Concatenate strings." },
    { label: "contains", kind: "function", detail: "string.contains(s, sub)", info: "Substring test." },
    { label: "startsWith", kind: "function", detail: "string.startsWith(s, prefix)", info: "Prefix test." },
    { label: "endsWith", kind: "function", detail: "string.endsWith(s, suffix)", info: "Suffix test." },
    { label: "split", kind: "function", detail: "string.split(s, sep)", info: "Split by separator." },
    { label: "trim", kind: "function", detail: "string.trim(s)", info: "Trim surrounding whitespace." },
    { label: "replace", kind: "function", detail: "string.replace(s, from, to)", info: "Substring replace." },
    { label: "capitalize", kind: "function", detail: "string.capitalize(s)", info: "Capitalise first letter." },
    { label: "camel", kind: "function", detail: "string.camel(s)", info: "Convert to camelCase." },
    { label: "count", kind: "function", detail: "string.count(s, sub)", info: "Occurrences of `sub`." },
  ],
  list: [
    { label: "sum", kind: "function", detail: "list.sum(xs)", info: "Sum of numeric items." },
    { label: "avg", kind: "function", detail: "list.avg(xs)", info: "Average of numeric items." },
    { label: "size", kind: "function", detail: "list.size(xs)", info: "Number of items." },
    { label: "append", kind: "function", detail: "list.append(xs, item)", info: "Append an item." },
    { label: "first", kind: "function", detail: "list.first(xs)", info: "First item." },
    { label: "last", kind: "function", detail: "list.last(xs)", info: "Last item." },
    { label: "contains", kind: "function", detail: "list.contains(xs, item)", info: "Membership test." },
    { label: "reverse", kind: "function", detail: "list.reverse(xs)", info: "Reverse." },
  ],
  map: [
    { label: "keys", kind: "function", detail: "map.keys(m)", info: "Keys of the map." },
    { label: "size", kind: "function", detail: "map.size(m)", info: "Number of entries." },
  ],
  temporal: [
    { label: "now", kind: "function", detail: "temporal.now()", info: "Current timestamp." },
    { label: "date", kind: "function", detail: "temporal.date(...)", info: "Construct a date." },
    { label: "datetime", kind: "function", detail: "temporal.datetime(...)", info: "Construct a datetime." },
  ],
};

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

/** Look up a token by its identifier (case-insensitive). */
export function findToken(name: string): CypherToken | undefined {
  return (
    ALL_KEYS.get(name.toUpperCase()) ??
    ALL_KEYS.get(name.toLowerCase())
  );
}
