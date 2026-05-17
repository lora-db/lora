"use client";

/**
 * Live schema introspection against the WASM database.
 *
 * Produces a {@link SchemaSnapshot} by probing the graph for labels,
 * relationship types, property keys, and per-label node counts. Each
 * "family" of facts is fetched via a stored procedure when available
 * and falls back to a plain Cypher aggregate otherwise. All failures
 * are swallowed — a missing or stubborn family yields an empty array
 * rather than aborting the whole snapshot.
 *
 * The functions here intentionally never throw: callers can treat the
 * returned snapshot as the source of truth and re-introspect on
 * demand (e.g. after a mutation).
 */

import { run } from "@/lib/db/client";
import type { RunOk } from "@/lib/db/types";
import type { SchemaSnapshot } from "@/lib/state/slices/schema";

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/**
 * Run a query and return the {@link RunOk} payload, or `null` if the
 * query errored or produced no rows. Never throws — exceptions inside
 * `run` are already normalised to a `RunErr`.
 */
async function runOk(body: string): Promise<RunOk | null> {
  try {
    const outcome = await run(body);
    if (outcome.state !== "ok") return null;
    if (outcome.result.rows.length === 0) return null;
    return outcome;
  } catch {
    return null;
  }
}

/**
 * Pull a column of strings out of an OK run. The column is looked up by
 * name when provided, otherwise the first column is used. Non-string /
 * empty entries are dropped, and the result is deduped while preserving
 * first-seen order.
 */
function pickStringColumn(outcome: RunOk, column?: string): string[] {
  const columns = outcome.result.columns;
  let columnIndex = 0;
  if (column !== undefined) {
    const idx = columns.indexOf(column);
    if (idx >= 0) columnIndex = idx;
  }
  const seen = new Set<string>();
  const out: string[] = [];
  for (const row of outcome.result.rows) {
    const value = row.values[columnIndex];
    if (typeof value !== "string" || value.length === 0) continue;
    if (seen.has(value)) continue;
    seen.add(value);
    out.push(value);
  }
  return out;
}

/**
 * Try `primary`, then `fallback`, returning the first non-empty result.
 * Used to keep procedure-vs-Cypher fan-out compact below.
 */
async function tryStringList(
  primary: { query: string; column?: string },
  fallback: { query: string; column?: string },
): Promise<string[]> {
  const primaryOk = await runOk(primary.query);
  if (primaryOk) {
    const list = pickStringColumn(primaryOk, primary.column);
    if (list.length > 0) return list;
  }
  const fallbackOk = await runOk(fallback.query);
  if (!fallbackOk) return [];
  return pickStringColumn(fallbackOk, fallback.column);
}

/**
 * Build a `{ label -> count }` map from a query that yields rows shaped
 * like `{ label: string, c: number }`. Coerces unexpected counts to 0
 * and skips rows whose label is empty / non-string.
 */
async function fetchCountsByLabel(): Promise<Record<string, number>> {
  const outcome = await runOk(
    "MATCH (n) UNWIND labels(n) AS l RETURN l AS label, count(*) AS c",
  );
  if (!outcome) return {};

  const labelIdx = outcome.result.columns.indexOf("label");
  const countIdx = outcome.result.columns.indexOf("c");
  if (labelIdx < 0 || countIdx < 0) return {};

  const out: Record<string, number> = {};
  for (const row of outcome.result.rows) {
    const label = row.values[labelIdx];
    const count = row.values[countIdx];
    if (typeof label !== "string" || label.length === 0) continue;
    const n = typeof count === "number" && Number.isFinite(count) ? count : 0;
    out[label] = n;
  }
  return out;
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * Maximum number of labels / rel-types to probe with a dedicated
 * per-name `keys(...)` query. Each probe is one extra Cypher
 * roundtrip, so on a schema with hundreds of labels we'd dominate
 * the snapshot latency. Beyond the cap we leave the entry empty —
 * the UI falls back to the global property-keys list.
 */
const PROPERTY_PROBE_CAP = 50;

/**
 * Probe each name in `names` for the property keys that appear on
 * matching nodes or relationships. Built around the same
 * try/catch-and-empty-array pattern as the rest of this file: a
 * failing probe yields `[]` rather than aborting the whole map.
 *
 * The label/relType is wrapped in backticks so names with spaces,
 * digits, or punctuation still parse. We don't escape further —
 * labels containing backticks themselves are exceedingly rare and
 * the resulting Cypher syntax error simply lands in the catch.
 */
async function fetchPropertiesByName(
  names: string[],
  buildQuery: (escaped: string) => string,
  kind: "label" | "relType",
): Promise<Record<string, string[]>> {
  const out: Record<string, string[]> = {};
  if (names.length === 0) return out;

  const capped = names.slice(0, PROPERTY_PROBE_CAP);
  if (names.length > PROPERTY_PROBE_CAP) {
    console.warn(
      `[schema] skipping per-${kind} property introspection beyond ${PROPERTY_PROBE_CAP} ` +
        `(${names.length - PROPERTY_PROBE_CAP} more will fall back to the global list)`,
    );
  }

  // Fan out the probes concurrently — each one is a small query and
  // the WASM client serialises them internally, so the wall-clock
  // cost is roughly N * (single probe). Promise.all keeps the
  // bookkeeping tidy.
  const probes = capped.map(async (name) => {
    const escaped = name.replace(/`/g, "");
    const outcome = await runOk(buildQuery(escaped));
    if (!outcome) return [name, [] as string[]] as const;
    const keys = pickStringColumn(outcome, "key");
    return [name, keys] as const;
  });

  const settled = await Promise.all(probes);
  for (const [name, keys] of settled) {
    out[name] = keys;
  }
  return out;
}

/**
 * Build a fresh {@link SchemaSnapshot} from the live database. Always
 * resolves — on total failure the returned snapshot is empty (`labels`
 * = `[]` etc.) with a current `fetchedAt` stamp.
 */
export async function introspect(): Promise<SchemaSnapshot> {
  // Run the four "global" probes in parallel so the worst-case
  // latency is the slowest single query rather than the sum of all
  // four. The per-label/per-relType probes need labels/relTypes
  // first, so they run as a second wave.
  const [labels, relTypes, propertyKeys, countsByLabel] = await Promise.all([
    tryStringList(
      {
        query: "CALL db.labels() YIELD label RETURN label",
        column: "label",
      },
      {
        query: "MATCH (n) UNWIND labels(n) AS l RETURN DISTINCT l",
      },
    ),
    tryStringList(
      {
        query:
          "CALL db.relationshipTypes() YIELD relationshipType RETURN relationshipType",
        column: "relationshipType",
      },
      {
        query: "MATCH ()-[r]->() RETURN DISTINCT type(r) AS t",
        column: "t",
      },
    ),
    tryStringList(
      {
        query: "CALL db.propertyKeys() YIELD propertyKey RETURN propertyKey",
        column: "propertyKey",
      },
      {
        // UNION between node + relationship key sources so we don't miss
        // keys that only exist on rels. UNION dedupes in Cypher.
        query:
          "MATCH (n) UNWIND keys(n) AS k RETURN DISTINCT k " +
          "UNION " +
          "MATCH ()-[r]->() UNWIND keys(r) AS k RETURN DISTINCT k",
        column: "k",
      },
    ),
    fetchCountsByLabel().catch(() => ({}) as Record<string, number>),
  ]);

  const [propertiesByLabel, propertiesByRelType] = await Promise.all([
    fetchPropertiesByName(
      labels,
      (escaped) =>
        `MATCH (n:\`${escaped}\`) UNWIND keys(n) AS k RETURN DISTINCT k AS key`,
      "label",
    ),
    fetchPropertiesByName(
      relTypes,
      (escaped) =>
        `MATCH ()-[r:\`${escaped}\`]->() UNWIND keys(r) AS k RETURN DISTINCT k AS key`,
      "relType",
    ),
  ]);

  return {
    labels,
    relTypes,
    propertyKeys,
    countsByLabel,
    propertiesByLabel,
    propertiesByRelType,
    fetchedAt: Date.now(),
  };
}
