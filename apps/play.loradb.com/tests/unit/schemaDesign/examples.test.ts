/**
 * Snapshot the usage examples we surface on the wizard's confirmation
 * step. The output is shown verbatim (after `formatSync`), so any
 * regression in the builder is user-visible.
 */

import { describe, expect, it } from "vitest";

import {
  buildConstraintUsageExamples,
  buildIndexUsageExamples,
} from "@/lib/schemaDesign/examples";
import type {
  ConstraintDraft,
  IndexDraft,
} from "@/lib/schemaDesign/types";

const indexDraft: IndexDraft = {
  kind: "RANGE",
  entity: "NODE",
  label: "Person",
  properties: ["email"],
  name: "idx_person_email",
  ifNotExists: true,
};

describe("buildIndexUsageExamples", () => {
  it("returns equality / range / sort snippets for RANGE", () => {
    const examples = buildIndexUsageExamples(indexDraft);
    expect(examples.map((e) => e.caption)).toEqual([
      "Equality lookup",
      "Range filter",
      "Sorted lookup",
    ]);
    expect(examples[0]!.cypher).toContain("WHERE n.`email` = $value");
  });

  it("returns prefix / substring snippets for TEXT", () => {
    const examples = buildIndexUsageExamples({ ...indexDraft, kind: "TEXT" });
    expect(examples.map((e) => e.caption)).toEqual([
      "Prefix match",
      "Substring match",
    ]);
    expect(examples[0]!.cypher).toContain("STARTS WITH $prefix");
  });

  it("returns a distance snippet for POINT", () => {
    const examples = buildIndexUsageExamples({
      ...indexDraft,
      kind: "POINT",
      properties: ["location"],
    });
    expect(examples).toHaveLength(1);
    expect(examples[0]!.cypher).toContain("point.distance(n.`location`");
  });

  it("returns a label-scan snippet for LOOKUP nodes using the hint", () => {
    const examples = buildIndexUsageExamples(
      {
        ...indexDraft,
        kind: "LOOKUP",
        label: "",
        properties: [],
      },
      { sampleLabel: "Movie" },
    );
    expect(examples).toHaveLength(1);
    expect(examples[0]!.cypher).toContain("MATCH (n:`Movie`)");
  });

  it("returns a fulltext call for FULLTEXT", () => {
    const examples = buildIndexUsageExamples({
      ...indexDraft,
      kind: "FULLTEXT",
      properties: ["bio"],
      name: "ft_person_bio",
    });
    expect(examples[0]!.cypher).toContain(
      "CALL db.index.fulltext.queryNodes('ft_person_bio'",
    );
  });

  it("returns nothing for non-LOOKUP drafts without label or properties", () => {
    expect(
      buildIndexUsageExamples({ ...indexDraft, label: "" }),
    ).toEqual([]);
    expect(
      buildIndexUsageExamples({ ...indexDraft, properties: [] }),
    ).toEqual([]);
  });
});

const constraintDraft: ConstraintDraft = {
  kind: "UNIQUE",
  entity: "NODE",
  label: "Person",
  properties: ["email"],
  propertyType: "STRING",
  name: "unique_person_email",
  ifNotExists: true,
};

describe("buildConstraintUsageExamples", () => {
  it("returns MERGE + rejected CREATE for UNIQUE on a node", () => {
    const examples = buildConstraintUsageExamples(constraintDraft);
    expect(examples).toHaveLength(2);
    expect(examples[0]!.cypher).toContain(
      "MERGE (n:`Person` {`email`: $email})",
    );
  });

  it("returns valid + rejected writes for NOT_NULL", () => {
    const examples = buildConstraintUsageExamples({
      ...constraintDraft,
      kind: "NOT_NULL",
    });
    expect(examples.map((e) => e.caption)).toEqual([
      "Valid write",
      "Missing email will be rejected",
    ]);
  });

  it("returns a typed write for PROPERTY_TYPE INTEGER", () => {
    const examples = buildConstraintUsageExamples({
      ...constraintDraft,
      kind: "PROPERTY_TYPE",
      propertyType: "INTEGER",
    });
    expect(examples[0]!.cypher).toContain("SET n.`email` = 42");
  });

  it("returns nothing when there is no label or property", () => {
    expect(
      buildConstraintUsageExamples({ ...constraintDraft, label: "" }),
    ).toEqual([]);
    expect(
      buildConstraintUsageExamples({ ...constraintDraft, properties: [] }),
    ).toEqual([]);
  });
});
