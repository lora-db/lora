/**
 * Heuristic recommendation engine. Scans a list of historical query
 * bodies for common patterns and surfaces "you probably want to index
 * X" / "you treat Y as a key" suggestions.
 *
 * Pure tokenizer-based, no full Cypher parse. We intentionally
 * tolerate false positives over false negatives — the user can
 * dismiss recommendations and missing one is more annoying than
 * showing an extra.
 */

import type {
  ConstraintDef,
  EntityKind,
  IndexDef,
  Recommendation,
  RecommendationKind,
} from "./types";

interface HistoryLike {
  body: string;
  ok: boolean;
}

interface ExistingState {
  indexes: ReadonlyArray<IndexDef>;
  constraints: ReadonlyArray<ConstraintDef>;
  dismissed: ReadonlySet<string>;
}

/**
 * Minimum number of separate query bodies that must match a pattern
 * before we surface a recommendation. Configurable for tests.
 */
export const DEFAULT_EVIDENCE_THRESHOLD = 3;

/** Stable id for a recommendation so the prefs slice can dismiss it. */
export function recommendationId(
  kind: RecommendationKind,
  entity: EntityKind,
  label: string,
  property: string,
): string {
  return `${kind}::${entity}::${label}::${property}`;
}

// ---------------------------------------------------------------------------
// Pattern scanners
// ---------------------------------------------------------------------------

// Matches `(<var>:<Label>)` introducing a label-bound variable, or the
// abbreviated `(:Label)`. We collect both the variable name (when
// present) and the label so later passes can map property accesses
// back to a label.
const VAR_LABEL_RE =
  /\(\s*(?:([A-Za-z_][A-Za-z0-9_]*))?\s*:\s*([A-Za-z_][A-Za-z0-9_]*)\s*[){]/g;

// `<var>.<prop>` — captures the variable and the property.
const VAR_PROP_RE = /\b([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*)/g;

// Inline property-set form `(n:Label {prop: …})` — captures Label + first prop.
// Only the first property in the map is captured; that's enough for
// MERGE/MATCH-by-key heuristics.
const LABEL_INLINE_PROP_RE =
  /\(\s*[A-Za-z_][A-Za-z0-9_]*?\s*:\s*([A-Za-z_][A-Za-z0-9_]*)\s*\{\s*([A-Za-z_][A-Za-z0-9_]*)\s*:/g;

interface FilterEvidence {
  label: string;
  property: string;
  /** Whether we saw `=` (range index candidate) or `STARTS WITH` (text). */
  kind: "EQUALITY" | "RANGE" | "TEXT";
}

/** Walk a single body and extract every `WHERE n.prop OP …` we can match. */
function extractFilterEvidence(body: string): FilterEvidence[] {
  const out: FilterEvidence[] = [];

  // Build a varName → Label map by replaying VAR_LABEL_RE.
  const varToLabel = new Map<string, string>();
  let m: RegExpExecArray | null;
  VAR_LABEL_RE.lastIndex = 0;
  while ((m = VAR_LABEL_RE.exec(body)) !== null) {
    const variable = m[1];
    const label = m[2];
    if (variable && label) varToLabel.set(variable, label);
  }

  // Then walk every var.prop reference; when followed by an operator
  // we know is filter-shaped, record it.
  VAR_PROP_RE.lastIndex = 0;
  while ((m = VAR_PROP_RE.exec(body)) !== null) {
    const variable = m[1];
    const prop = m[2];
    if (!variable || !prop) continue;
    const label = varToLabel.get(variable);
    if (!label) continue;
    // Look at the next ~32 chars after the property access to decide
    // what operator (if any) follows.
    const tail = body.slice(m.index + m[0].length, m.index + m[0].length + 32);
    if (/^\s*(?:=|<>|!=)/.test(tail)) {
      out.push({ label, property: prop, kind: "EQUALITY" });
    } else if (/^\s*(?:>=?|<=?)/.test(tail)) {
      out.push({ label, property: prop, kind: "RANGE" });
    } else if (/^\s+(?:STARTS|ENDS|CONTAINS)\b/i.test(tail)) {
      out.push({ label, property: prop, kind: "TEXT" });
    }
  }

  return out;
}

interface MergeKeyEvidence {
  label: string;
  property: string;
}

/**
 * Find `MERGE (n:Label {prop: …})` patterns — the user is treating
 * `prop` as a natural key.
 */
function extractMergeKeyEvidence(body: string): MergeKeyEvidence[] {
  if (!/\bMERGE\b/i.test(body)) return [];
  const out: MergeKeyEvidence[] = [];
  LABEL_INLINE_PROP_RE.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = LABEL_INLINE_PROP_RE.exec(body)) !== null) {
    out.push({ label: m[1]!, property: m[2]! });
  }
  return out;
}

// ---------------------------------------------------------------------------
// Aggregation
// ---------------------------------------------------------------------------

type Key = string; // `${kind}::${label}::${property}`

interface Bucket {
  kind: RecommendationKind;
  entity: EntityKind;
  label: string;
  property: string;
  evidence: number;
}

function bumpBucket(
  buckets: Map<Key, Bucket>,
  kind: RecommendationKind,
  label: string,
  property: string,
): void {
  // The heuristics here only see node patterns — we don't try to infer
  // rel-type filters from history. So entity is always NODE for v1.
  const entity: EntityKind = "NODE";
  const key = `${kind}::${entity}::${label}::${property}`;
  const existing = buckets.get(key);
  if (existing) {
    existing.evidence += 1;
    return;
  }
  buckets.set(key, { kind, entity, label, property, evidence: 1 });
}

function alreadyCoveredByIndex(
  existing: ExistingState,
  kind: RecommendationKind,
  label: string,
  property: string,
): boolean {
  for (const idx of existing.indexes) {
    if (idx.labelsOrTypes[0] !== label) continue;
    if (!idx.properties.includes(property)) continue;
    if (kind === "RANGE_INDEX" && idx.kind === "RANGE") return true;
    if (kind === "TEXT_INDEX" && idx.kind === "TEXT") return true;
  }
  return false;
}

function alreadyCoveredByConstraint(
  existing: ExistingState,
  kind: RecommendationKind,
  label: string,
  property: string,
): boolean {
  for (const c of existing.constraints) {
    if (c.label !== label) continue;
    if (!c.properties.includes(property)) continue;
    if (
      kind === "UNIQUE_CONSTRAINT" &&
      (c.kind === "UNIQUE" || c.kind === "NODE_KEY")
    )
      return true;
    if (
      kind === "NOT_NULL_CONSTRAINT" &&
      (c.kind === "NOT_NULL" || c.kind === "NODE_KEY")
    )
      return true;
  }
  return false;
}

function reasonFor(
  kind: RecommendationKind,
  count: number,
  property: string,
): string {
  switch (kind) {
    case "RANGE_INDEX":
      return `Filtered by \`${property}\` in ${count} queries — a RANGE index would speed those up.`;
    case "TEXT_INDEX":
      return `Searched with STARTS WITH / CONTAINS / ENDS WITH on \`${property}\` in ${count} queries — a TEXT index fits.`;
    case "UNIQUE_CONSTRAINT":
      return `Treated \`${property}\` as a natural key (MERGE) in ${count} queries — a UNIQUE constraint enforces that.`;
    case "NOT_NULL_CONSTRAINT":
      return `Every query referenced \`${property}\` — likely a required field.`;
  }
}

export function generateRecommendations(
  history: ReadonlyArray<HistoryLike>,
  existing: ExistingState,
  opts?: { evidenceThreshold?: number },
): Recommendation[] {
  const threshold = opts?.evidenceThreshold ?? DEFAULT_EVIDENCE_THRESHOLD;
  const buckets = new Map<Key, Bucket>();

  for (const entry of history) {
    if (!entry.ok) continue;
    const body = entry.body;
    if (typeof body !== "string" || body.length === 0) continue;

    for (const ev of extractFilterEvidence(body)) {
      if (ev.kind === "EQUALITY" || ev.kind === "RANGE") {
        bumpBucket(buckets, "RANGE_INDEX", ev.label, ev.property);
      } else if (ev.kind === "TEXT") {
        bumpBucket(buckets, "TEXT_INDEX", ev.label, ev.property);
      }
    }
    for (const ev of extractMergeKeyEvidence(body)) {
      bumpBucket(buckets, "UNIQUE_CONSTRAINT", ev.label, ev.property);
    }
  }

  const out: Recommendation[] = [];
  for (const b of buckets.values()) {
    if (b.evidence < threshold) continue;
    const id = recommendationId(b.kind, b.entity, b.label, b.property);
    if (existing.dismissed.has(id)) continue;
    if (
      b.kind === "RANGE_INDEX" || b.kind === "TEXT_INDEX"
        ? alreadyCoveredByIndex(existing, b.kind, b.label, b.property)
        : alreadyCoveredByConstraint(existing, b.kind, b.label, b.property)
    ) {
      continue;
    }
    out.push({
      id,
      kind: b.kind,
      entity: b.entity,
      label: b.label,
      property: b.property,
      evidenceCount: b.evidence,
      reason: reasonFor(b.kind, b.evidence, b.property),
    });
  }
  // Highest evidence first, then alphabetical for determinism.
  out.sort(
    (a, b) =>
      b.evidenceCount - a.evidenceCount ||
      a.label.localeCompare(b.label) ||
      a.property.localeCompare(b.property),
  );
  return out;
}
