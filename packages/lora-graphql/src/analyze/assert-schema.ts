// The database side of S1: whether the database has every constraint and
// index the API needs (creating what is missing on request), and which
// of its indexes no part of the API uses.

import type { LoraDriver } from "../driver.js";
import type { AssertSchemaOptions } from "../options.js";
import type { CheckReport, SchemaAssertion } from "../results.js";
import { requirementDdl, type SchemaRequirement } from "./indexes.js";

export interface SchemaEnv {
  driver: LoraDriver;
  /** Timeout for every statement, in milliseconds. */
  timeoutMs: number;
  /** Constraints and indexes the API needs. */
  requirements(): SchemaRequirement[];
}

/**
 * Verify the database has every constraint and index the API needs;
 * with `create`, add what is missing. Idempotent. Runs one DDL
 * statement at a time: LoraDB rejects schema commands in transactions.
 */
export async function assertSchema(
  env: SchemaEnv,
  options: AssertSchemaOptions,
): Promise<SchemaAssertion> {
  const required = env.requirements();
  const [indexes, constraints] = await env.driver.run(
    [
      { text: "SHOW INDEXES", params: {} },
      { text: "SHOW CONSTRAINTS", params: {} },
    ],
    { mode: "read", timeoutMs: env.timeoutMs },
  );
  const byName = (r: SchemaRequirement) =>
    indexes!.rows.find((row) => row["name"] === r.name);
  // A named index whose definition differs from the model's: matching by
  // name alone would keep searching the old field list.
  const differs = (r: SchemaRequirement) => {
    if (r.kind !== "fulltext" && r.kind !== "vector") return false;
    const row = byName(r);
    if (!row) return false;
    const type = r.kind === "fulltext" ? "FULLTEXT" : "VECTOR";
    const wantedProps = r.kind === "fulltext" ? r.properties : [r.property];
    const same = (a: unknown, b: readonly string[]) =>
      Array.isArray(a) &&
      a.length === b.length &&
      [...a].map(String).sort().join("\0") === [...b].sort().join("\0");
    const analyzer = (row["options"] as Record<string, unknown> | undefined)?.[
      "fulltext.analyzer"
    ];
    return (
      row["type"] !== type ||
      !same(row["labelsOrTypes"], [r.label]) ||
      !same(row["properties"], wantedProps) ||
      (r.kind === "fulltext" &&
        typeof analyzer === "string" &&
        analyzer.toUpperCase() !== r.analyzer)
    );
  };
  const present = (r: SchemaRequirement) => {
    if (r.kind === "fulltext" || r.kind === "vector") {
      return byName(r) !== undefined;
    }
    if (r.kind === "constraint") {
      const wanted = {
        NODE_KEY: ["NODE_KEY"],
        UNIQUE: ["NODE_KEY", "NODE_PROPERTY_UNIQUENESS"],
        NOT_NULL: ["NODE_KEY", "NODE_PROPERTY_EXISTENCE"],
      }[r.constraint];
      return constraints!.rows.some(
        (row) =>
          wanted.includes(String(row["type"])) &&
          sameTarget(row, r.label, r.property),
      );
    }
    return indexes!.rows.some(
      (row) => row["type"] === r.index && sameTarget(row, r.label, r.property),
    );
  };
  const missing = required.filter((r) => !present(r));
  const mismatched = required.filter(differs);
  const created: SchemaRequirement[] = [];
  const recreated: SchemaRequirement[] = [];
  if (options.create) {
    for (const r of mismatched) {
      await env.driver.run(
        [
          { text: `DROP INDEX ${quoteName(r.name)}`, params: {} },
          { text: requirementDdl(r), params: {} },
        ],
        { mode: "write", timeoutMs: env.timeoutMs },
      );
      recreated.push(r);
    }
    for (const r of missing) {
      await env.driver.run([{ text: requirementDdl(r), params: {} }], {
        mode: "write",
        timeoutMs: env.timeoutMs,
      });
      created.push(r);
    }
  }
  return {
    required,
    missing: options.create ? [] : missing,
    created,
    mismatched: options.create ? [] : mismatched,
    recreated,
  };
}

/** Indexes in the database that no requirement of the API matches. */
export async function unusedIndexes(
  env: SchemaEnv,
): Promise<CheckReport["unused"]> {
  const [indexes, constraints] = await env.driver.run(
    [
      { text: "SHOW INDEXES", params: {} },
      { text: "SHOW CONSTRAINTS", params: {} },
    ],
    { mode: "read", timeoutMs: env.timeoutMs },
  );
  // A constraint's backing index serves the constraint: LoraDB names it
  // in the constraint's `ownedIndex`, not on the index row.
  const owned = new Set(
    constraints!.rows.map((row) => row["ownedIndex"]).filter(Boolean),
  );
  const required = env.requirements();
  return indexes!.rows
    .filter(
      (row) =>
        row["type"] !== "LOOKUP" &&
        !row["owningConstraint"] &&
        !owned.has(row["name"]),
    )
    .filter(
      (row) =>
        !required.some((r) =>
          r.kind === "fulltext" || r.kind === "vector"
            ? r.name === row["name"]
            : r.kind === "index" &&
              r.index === row["type"] &&
              sameTarget(row, r.label, r.property),
        ),
    )
    .map((row) => ({
      name: String(row["name"]),
      type: String(row["type"]),
      labels: (row["labelsOrTypes"] as string[] | null) ?? [],
      properties: (row["properties"] as string[] | null) ?? [],
    }));
}

/** An index name as Cypher takes it: backquoted. */
function quoteName(name: string): string {
  return "`" + name.replaceAll("`", "``") + "`";
}

function sameTarget(
  row: Record<string, unknown>,
  label: string,
  property: string,
): boolean {
  const labels = row["labelsOrTypes"] as string[] | null;
  const props = row["properties"] as string[] | null;
  return (
    row["entityType"] === "NODE" &&
    labels?.length === 1 &&
    labels[0] === label &&
    props?.length === 1 &&
    props[0] === property
  );
}
