/**
 * Cypher snippet generators driven by the schema panel.
 *
 * The schema browser hands users a one-click path from "I see this
 * label" to "a runnable starter query". Every helper here is a pure
 * string builder so callers can compose them freely and tests stay
 * trivial.
 *
 * Identifier escaping follows the same convention as the introspection
 * code in `lib/db/schema.ts`: wrap names in backticks and strip any
 * embedded backticks. Labels/rel-types with funky characters still
 * parse; the rare label that legitimately contains a backtick yields a
 * mangled but recognisable query rather than a syntax error.
 */

const SAMPLE_LIMIT = 25;

/**
 * Cypher identifiers (labels, rel-types, properties) are bare when they
 * match `[A-Za-z_][A-Za-z0-9_]*`, and need backticks otherwise (spaces,
 * dashes, leading digits, etc.). Generated queries should match what a
 * human would type — quoting `Venue` would just be visual noise.
 */
const PLAIN_ID = /^[A-Za-z_][A-Za-z0-9_]*$/;

/** Escape a Cypher identifier only when the grammar requires it. */
export function quoteId(name: string): string {
  if (PLAIN_ID.test(name)) return name;
  return `\`${name.replace(/`/g, "")}\``;
}

/** Lower-case first character — used to build a sensible binding name. */
function bindingFor(label: string): string {
  const cleaned = label.replace(/[^A-Za-z0-9_]/g, "");
  if (cleaned.length === 0) return "n";
  const first = cleaned[0]!.toLowerCase();
  return first;
}

export interface LabelSnippetOptions {
  /** When true, project a specific property instead of the whole node. */
  property?: string;
}

/** `MATCH (n:Label) RETURN n LIMIT 25` (or `n.prop` when `property` is set). */
export function labelMatch(
  label: string,
  opts: LabelSnippetOptions = {},
): string {
  const b = bindingFor(label);
  const projection = opts.property ? `${b}.${quoteId(opts.property)}` : b;
  return `MATCH (${b}:${quoteId(label)})\nRETURN ${projection}\nLIMIT ${SAMPLE_LIMIT}`;
}

/** `MATCH (n:Label) RETURN count(n)` — the canonical "how many?" probe. */
export function labelCount(label: string): string {
  const b = bindingFor(label);
  return `MATCH (${b}:${quoteId(label)})\nRETURN count(${b}) AS count`;
}

/** A single sample row — useful as a "what does one of these look like?" probe. */
export function labelSample(label: string): string {
  const b = bindingFor(label);
  return `MATCH (${b}:${quoteId(label)})\nRETURN ${b}\nLIMIT 1`;
}

/** Distinct values of a property on a labeled node. */
export function labelDistinctProperty(label: string, property: string): string {
  const b = bindingFor(label);
  return `MATCH (${b}:${quoteId(label)})\nRETURN DISTINCT ${b}.${quoteId(property)} AS ${quoteId(property)}\nORDER BY ${quoteId(property)}\nLIMIT ${SAMPLE_LIMIT}`;
}

/** A label and its immediate neighbors via any relationship. */
export function labelNeighbors(label: string): string {
  const b = bindingFor(label);
  return `MATCH (${b}:${quoteId(label)})-[r]-(m)\nRETURN ${b}, r, m\nLIMIT ${SAMPLE_LIMIT}`;
}

/** `MATCH ()-[r:REL_TYPE]->() RETURN r LIMIT 25`. */
export function relTypeMatch(relType: string): string {
  return `MATCH ()-[r:${quoteId(relType)}]->()\nRETURN r\nLIMIT ${SAMPLE_LIMIT}`;
}

/** Project a single property off a rel-type: `RETURN r.prop`. */
export function relTypeProjection(relType: string, property: string): string {
  return `MATCH ()-[r:${quoteId(relType)}]->()\nRETURN r.${quoteId(property)}\nLIMIT ${SAMPLE_LIMIT}`;
}

export function relTypeCount(relType: string): string {
  return `MATCH ()-[r:${quoteId(relType)}]->()\nRETURN count(r) AS count`;
}

/** A rel-type with both endpoints — the most-asked "show me an edge" query. */
export function relTypeEndpoints(relType: string): string {
  return `MATCH (a)-[r:${quoteId(relType)}]->(b)\nRETURN a, r, b\nLIMIT ${SAMPLE_LIMIT}`;
}

/** Distinct values of a property on a relationship. */
export function relTypeDistinctProperty(
  relType: string,
  property: string,
): string {
  return `MATCH ()-[r:${quoteId(relType)}]->()\nRETURN DISTINCT r.${quoteId(property)} AS ${quoteId(property)}\nORDER BY ${quoteId(property)}\nLIMIT ${SAMPLE_LIMIT}`;
}

/**
 * Distinct values for a property across any node that carries it —
 * used when the property is clicked from the flat property-keys list
 * (no label/rel-type context).
 */
export function propertyDistinctAny(property: string): string {
  return `MATCH (n)\nWHERE n.${quoteId(property)} IS NOT NULL\nRETURN DISTINCT n.${quoteId(property)} AS ${quoteId(property)}\nORDER BY ${quoteId(property)}\nLIMIT ${SAMPLE_LIMIT}`;
}
