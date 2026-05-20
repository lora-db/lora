"use client";

/**
 * Pre-flight scans for the constraint wizard. Run a non-destructive
 * Cypher query against the live graph to see whether the constraint
 * would fail on existing data. Each scan returns a structured
 * verdict + a sample of offending rows the UI can jump to.
 *
 * Each scan starts with a `capacity` count query. When the universe
 * is bigger than {@link SCAN_LIMIT} we refuse to run the full scan
 * (so we never block the WASM worker on huge labels) and return a
 * verdict with `capped: true` so the UI can disclose it and offer
 * the jump query for the user to run manually.
 */

import { run } from "@/lib/db/client";
import type { ConstraintDraft } from "./types";

const SCAN_LIMIT = 100_000;
const SAMPLE_LIMIT = 5;

export interface PreflightVerdict {
  ok: boolean;
  /** Total offending records observed (within the scan window). */
  offending: number;
  /** Whether the underlying scan was capped at SCAN_LIMIT. */
  capped: boolean;
  /** Sample rows the UI can show; columns vary by kind. */
  sample: ReadonlyArray<Record<string, unknown>>;
  /** A jumpable Cypher query that returns the full offending set. */
  jumpQuery: string;
  /** Short human-readable verdict for the wizard step. */
  message: string;
}

function backtick(id: string): string {
  return "`" + id.replace(/`/g, "") + "`";
}

function patternFor(draft: ConstraintDraft): string {
  return draft.entity === "NODE"
    ? `(n:${backtick(draft.label)})`
    : `()-[n:${backtick(draft.label)}]-()`;
}

/**
 * Read a numeric column from the single row of a count-style result.
 * Falls back to 0 when the shape isn't what we expected.
 */
function readNumberColumn(
  outcome: Extract<Awaited<ReturnType<typeof run>>, { state: "ok" }>,
  column: string,
): number {
  const row = outcome.result.rows[0];
  const idx = outcome.result.columns.indexOf(column);
  if (!row || idx < 0) return 0;
  const v = row.values[idx];
  return typeof v === "number" ? v : 0;
}

/**
 * Count how many rows match the constraint's universe. Used to bail
 * out before running a full scan on a label that's too big.
 */
async function scanCapacity(
  pattern: string,
): Promise<{ total: number; capped: boolean; error?: string }> {
  const outcome = await run(
    `MATCH ${pattern} RETURN count(*) AS total LIMIT 1`,
  );
  if (outcome.state !== "ok") {
    return {
      total: 0,
      capped: false,
      error:
        outcome.state === "error" ? outcome.message : "Capacity scan failed.",
    };
  }
  const total = readNumberColumn(outcome, "total");
  return { total, capped: total > SCAN_LIMIT };
}

/** Pre-flight scan for UNIQUE / NODE KEY / RELATIONSHIP KEY. */
async function scanUniqueness(
  draft: ConstraintDraft,
): Promise<PreflightVerdict> {
  const pattern = patternFor(draft);
  const groupAlias = "k";
  const aliasAssignments =
    draft.properties.length === 1
      ? `n.${backtick(draft.properties[0]!)} AS ${groupAlias}`
      : draft.properties
          .map((p, i) => `n.${backtick(p)} AS ${groupAlias}${i}`)
          .join(", ");
  const groupBy =
    draft.properties.length === 1
      ? groupAlias
      : draft.properties.map((_, i) => `${groupAlias}${i}`).join(", ");

  const jumpQuery =
    `// duplicates that would block this UNIQUE / KEY constraint\n` +
    `MATCH ${pattern}\n` +
    `WITH ${aliasAssignments}, count(*) AS c\n` +
    `WHERE c > 1\n` +
    `RETURN ${groupBy}, c\n` +
    `ORDER BY c DESC`;

  const capacity = await scanCapacity(pattern);
  if (capacity.error) {
    return {
      ok: false,
      offending: 0,
      capped: false,
      sample: [],
      jumpQuery,
      message: capacity.error,
    };
  }
  if (capacity.capped) {
    return {
      ok: false,
      offending: 0,
      capped: true,
      sample: [],
      jumpQuery,
      message:
        `Universe is ${capacity.total.toLocaleString()} rows ` +
        `(scan cap ${SCAN_LIMIT.toLocaleString()}). ` +
        `Open “View offending rows” to verify manually.`,
    };
  }

  const fullQuery =
    `MATCH ${pattern} ` +
    `WITH ${aliasAssignments}, count(*) AS c ` +
    `WHERE c > 1 ` +
    `RETURN ${groupBy}, c ` +
    `ORDER BY c DESC ` +
    `LIMIT ${SAMPLE_LIMIT}`;

  const countQuery =
    `MATCH ${pattern} ` +
    `WITH ${aliasAssignments}, count(*) AS c ` +
    `WHERE c > 1 ` +
    `RETURN count(*) AS offending ` +
    `LIMIT 1`;

  const [sampleOutcome, countOutcome] = await Promise.all([
    run(fullQuery),
    run(countQuery),
  ]);

  if (sampleOutcome.state !== "ok" || countOutcome.state !== "ok") {
    const err =
      sampleOutcome.state === "error"
        ? sampleOutcome.message
        : "Pre-flight scan failed.";
    return {
      ok: false,
      offending: 0,
      capped: false,
      sample: [],
      jumpQuery,
      message: err,
    };
  }

  const sample = sampleOutcome.result.rows.map((r) => {
    const obj: Record<string, unknown> = {};
    sampleOutcome.result.columns.forEach((col, i) => {
      obj[col] = r.values[i];
    });
    return obj;
  });

  const offending = readNumberColumn(countOutcome, "offending");

  return {
    ok: offending === 0,
    offending,
    capped: false,
    sample,
    jumpQuery,
    message:
      offending === 0
        ? "No duplicate values found — UNIQUE / KEY constraint can be added cleanly."
        : `${offending} group${offending === 1 ? "" : "s"} of duplicate values already exist. Resolve them before continuing.`,
  };
}

/** Pre-flight scan for IS NOT NULL. */
async function scanExistence(
  draft: ConstraintDraft,
): Promise<PreflightVerdict> {
  const pattern = patternFor(draft);
  const prop = draft.properties[0]!;
  const jumpQuery =
    `// records missing the required property\n` +
    `MATCH ${pattern}\n` +
    `WHERE n.${backtick(prop)} IS NULL\n` +
    `RETURN n`;

  const capacity = await scanCapacity(pattern);
  if (capacity.error) {
    return {
      ok: false,
      offending: 0,
      capped: false,
      sample: [],
      jumpQuery,
      message: capacity.error,
    };
  }
  if (capacity.capped) {
    return {
      ok: false,
      offending: 0,
      capped: true,
      sample: [],
      jumpQuery,
      message:
        `Universe is ${capacity.total.toLocaleString()} rows ` +
        `(scan cap ${SCAN_LIMIT.toLocaleString()}). ` +
        `Open “View offending rows” to verify manually.`,
    };
  }

  const sampleQuery =
    `MATCH ${pattern} ` +
    `WHERE n.${backtick(prop)} IS NULL ` +
    `RETURN n ` +
    `LIMIT ${SAMPLE_LIMIT}`;
  const countQuery =
    `MATCH ${pattern} ` +
    `WHERE n.${backtick(prop)} IS NULL ` +
    `RETURN count(*) AS offending ` +
    `LIMIT 1`;

  const [sampleOutcome, countOutcome] = await Promise.all([
    run(sampleQuery),
    run(countQuery),
  ]);

  if (sampleOutcome.state !== "ok" || countOutcome.state !== "ok") {
    const err =
      sampleOutcome.state === "error"
        ? sampleOutcome.message
        : "Pre-flight scan failed.";
    return {
      ok: false,
      offending: 0,
      capped: false,
      sample: [],
      jumpQuery,
      message: err,
    };
  }

  const sample = sampleOutcome.result.rows.map((r) => {
    const obj: Record<string, unknown> = {};
    sampleOutcome.result.columns.forEach((col, i) => {
      obj[col] = r.values[i];
    });
    return obj;
  });

  const offending = readNumberColumn(countOutcome, "offending");

  return {
    ok: offending === 0,
    offending,
    capped: false,
    sample,
    jumpQuery,
    message:
      offending === 0
        ? "Every record already has this property — IS NOT NULL can be added cleanly."
        : `${offending} record${offending === 1 ? "" : "s"} are missing this property. Backfill them before continuing.`,
  };
}

/** Pre-flight scan for IS :: TYPE. */
async function scanPropertyType(
  draft: ConstraintDraft,
): Promise<PreflightVerdict> {
  const pattern = patternFor(draft);
  const prop = draft.properties[0]!;
  const jumpQuery =
    `// values that don't satisfy the type predicate\n` +
    `MATCH ${pattern}\n` +
    `WHERE n.${backtick(prop)} IS NOT NULL\n` +
    `  AND NOT (n.${backtick(prop)} IS :: ${draft.propertyType})\n` +
    `RETURN n.${backtick(prop)} AS value`;

  const capacity = await scanCapacity(pattern);
  if (capacity.error) {
    return {
      ok: false,
      offending: 0,
      capped: false,
      sample: [],
      jumpQuery,
      message: capacity.error,
    };
  }
  if (capacity.capped) {
    return {
      ok: false,
      offending: 0,
      capped: true,
      sample: [],
      jumpQuery,
      message:
        `Universe is ${capacity.total.toLocaleString()} rows ` +
        `(scan cap ${SCAN_LIMIT.toLocaleString()}). ` +
        `Open “View offending rows” to verify manually.`,
    };
  }

  const sampleQuery =
    `MATCH ${pattern} ` +
    `WHERE n.${backtick(prop)} IS NOT NULL AND NOT (n.${backtick(prop)} IS :: ${draft.propertyType}) ` +
    `RETURN n.${backtick(prop)} AS value ` +
    `LIMIT ${SAMPLE_LIMIT}`;
  const countQuery =
    `MATCH ${pattern} ` +
    `WHERE n.${backtick(prop)} IS NOT NULL AND NOT (n.${backtick(prop)} IS :: ${draft.propertyType}) ` +
    `RETURN count(*) AS offending ` +
    `LIMIT 1`;

  const [sampleOutcome, countOutcome] = await Promise.all([
    run(sampleQuery),
    run(countQuery),
  ]);

  if (sampleOutcome.state !== "ok" || countOutcome.state !== "ok") {
    const err =
      sampleOutcome.state === "error"
        ? sampleOutcome.message
        : "Pre-flight scan failed.";
    return {
      ok: false,
      offending: 0,
      capped: false,
      sample: [],
      jumpQuery,
      message: err,
    };
  }

  const sample = sampleOutcome.result.rows.map((r) => {
    const obj: Record<string, unknown> = {};
    sampleOutcome.result.columns.forEach((col, i) => {
      obj[col] = r.values[i];
    });
    return obj;
  });

  const offending = readNumberColumn(countOutcome, "offending");

  return {
    ok: offending === 0,
    offending,
    capped: false,
    sample,
    jumpQuery,
    message:
      offending === 0
        ? `No values violate IS :: ${draft.propertyType} — safe to add.`
        : `${offending} value${offending === 1 ? "" : "s"} don't match ${draft.propertyType}. Convert or remove them first.`,
  };
}

/** Dispatch to the right scanner for the draft's kind. */
export async function runPreflight(
  draft: ConstraintDraft,
): Promise<PreflightVerdict> {
  switch (draft.kind) {
    case "UNIQUE":
    case "NODE_KEY":
    case "RELATIONSHIP_KEY":
      return scanUniqueness(draft);
    case "NOT_NULL":
      return scanExistence(draft);
    case "PROPERTY_TYPE":
      return scanPropertyType(draft);
  }
}

export { SCAN_LIMIT };
