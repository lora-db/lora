// Stored values against the validation rules. A rule is checked when a
// value is written through the API: one added or tightened later does
// not revisit what is already in the database, and raw Cypher is never
// checked. This reads the nodes of every type that has rules and reports
// the values that would be refused today, with the same code that
// refuses them on write. It also reports values a full-text index cannot
// hold: the engine indexes strings and skips anything else without a
// word, so a number written by raw Cypher is simply never found.

import { name } from "../compile/cypher.js";
import type { LoraDriver } from "../driver.js";
import { checkValue, normalizeValue } from "../model/constraints.js";
import type { GraphModel, ScalarField } from "../model/types.js";

/** A stored value that breaks a rule of its field. */
export interface StoredViolation {
  type: string;
  /** The node's `@key`; for a relationship property, the declaring node's. */
  key: unknown;
  field: string;
  /**
   * The rule, as in a request's `violations`; `normalize` for a value not
   * in its stored form; `fulltext` for one a full-text index skips.
   */
  rule: string;
  message: string;
}

export interface StoredReport {
  violations: StoredViolation[];
  /**
   * Nodes read per type, and relationships per `Owner.field` whose
   * properties have rules; one with more than `limit` is read in part.
   */
  scanned: Record<string, number>;
  /** Types with more nodes than were read. */
  partial: string[];
}

/** Most violations one report lists. */
const MAX_REPORTED = 200;

export async function scanStored(
  driver: LoraDriver,
  model: GraphModel,
  options: { limit?: number; timeoutMs?: number } = {},
): Promise<StoredReport> {
  const limit = options.limit ?? 10_000;
  const report: StoredReport = { violations: [], scanned: {}, partial: [] };
  for (const node of model.nodes.values()) {
    // Fields of a full-text index, by the index that holds them.
    const indexed = new Map<ScalarField, string>();
    for (const index of node.search) {
      if (index.kind !== "fulltext") continue;
      for (const f of index.fields) indexed.set(f, index.name);
    }
    const fields = [...node.fields.values()].filter(
      (f): f is ScalarField =>
        f.kind === "scalar" && (!!f.constraints || indexed.has(f)),
    );
    if (fields.length === 0) continue;
    const key = node.key.property;
    const [result] = await driver.run(
      [
        {
          text:
            `MATCH (n:${name(node.labels[0]!)})\n` +
            `RETURN n.${name(key)} AS key, ${fields.map((f, i) => `n.${name(f.property)} AS f${i}`).join(", ")}\n` +
            `ORDER BY n.${name(key)} LIMIT $limit`,
          params: { limit: limit + 1 },
        },
      ],
      { mode: "read", timeoutMs: options.timeoutMs },
    );
    const rows = result?.rows ?? [];
    if (rows.length > limit) report.partial.push(node.name);
    report.scanned[node.name] = Math.min(rows.length, limit);
    for (const row of rows.slice(0, limit)) {
      for (const [i, f] of fields.entries()) {
        const value = row[`f${i}`];
        if (value === null || value === undefined) continue;
        const found: Array<{ rule: string; message: string }> = [];
        const index = indexed.get(f);
        if (
          index !== undefined &&
          (Array.isArray(value) ? value : [value]).some(
            (item) => typeof item !== "string",
          )
        ) {
          found.push({
            rule: "fulltext",
            message: `is not text: the full-text index ${index} skips it, so a search never finds the node by this value`,
          });
        }
        found.push(...violationsOf(f, value));
        for (const { rule, message } of found) {
          if (report.violations.length >= MAX_REPORTED) continue;
          report.violations.push({
            type: node.name,
            key: row["key"],
            field: f.name,
            rule,
            message,
          });
        }
      }
    }
  }
  // Relationship properties with rules: each relationship type once,
  // from the side that declares it, named by that node's key.
  const seen = new Set<string>();
  for (const node of model.nodes.values()) {
    for (const rel of node.fields.values()) {
      if (rel.kind !== "relationship" || !rel.properties) continue;
      const id = `${rel.type}\0${rel.properties}`;
      if (seen.has(id)) continue;
      const fields = [
        ...(model.relationshipProperties.get(rel.properties)?.fields.values() ??
          []),
      ].filter((f) => !!f.constraints);
      if (fields.length === 0) continue;
      seen.add(id);
      const pattern =
        rel.direction === "OUT"
          ? `(a:${name(node.labels[0]!)})-[r:${name(rel.type)}]->()`
          : `(a:${name(node.labels[0]!)})<-[r:${name(rel.type)}]-()`;
      const [result] = await driver.run(
        [
          {
            text:
              `MATCH ${pattern}\n` +
              `RETURN a.${name(node.key.property)} AS key, ${fields.map((f, i) => `r.${name(f.property)} AS f${i}`).join(", ")}\n` +
              `LIMIT $limit`,
            params: { limit: limit + 1 },
          },
        ],
        { mode: "read", timeoutMs: options.timeoutMs },
      );
      const rows = result?.rows ?? [];
      const scope = `${node.name}.${rel.name}`;
      if (rows.length > limit) report.partial.push(scope);
      report.scanned[scope] = Math.min(rows.length, limit);
      for (const row of rows.slice(0, limit)) {
        for (const [i, f] of fields.entries()) {
          const value = row[`f${i}`];
          if (value === null || value === undefined) continue;
          for (const { rule, message } of violationsOf(f, value)) {
            if (report.violations.length >= MAX_REPORTED) continue;
            report.violations.push({
              type: node.name,
              key: row["key"],
              field: `${rel.name}.${f.name}`,
              rule,
              message,
            });
          }
        }
      }
    }
  }
  return report;
}

/** What a stored value breaks of its field's rules. */
function violationsOf(
  f: ScalarField,
  value: unknown,
): Array<{ rule: string; message: string }> {
  const found: Array<{ rule: string; message: string }> = [];
  if (!f.constraints) return found;
  if (
    f.constraints.normalize &&
    JSON.stringify(normalizeValue(f.constraints, value)) !==
      JSON.stringify(value)
  ) {
    found.push({
      rule: "normalize",
      message: "is not in the form @normalize stores",
    });
  }
  found.push(...checkValue(f.constraints, value));
  return found;
}
