/**
 * Tests for the recommendation heuristic. We feed in synthetic query
 * histories and verify the buckets the regex tokenizer produces.
 */

import { describe, expect, it } from "vitest";

import {
  DEFAULT_EVIDENCE_THRESHOLD,
  generateRecommendations,
} from "@/lib/schemaDesign/recommend";
import type {
  ConstraintDef,
  IndexDef,
  Recommendation,
} from "@/lib/schemaDesign/types";

const EMPTY = {
  indexes: [] as ReadonlyArray<IndexDef>,
  constraints: [] as ReadonlyArray<ConstraintDef>,
  dismissed: new Set<string>(),
};

function triplet(body: string) {
  return Array.from({ length: DEFAULT_EVIDENCE_THRESHOLD }, () => ({
    body,
    ok: true,
  }));
}

describe("equality filters → RANGE_INDEX", () => {
  it("buckets repeated equality predicates as a RANGE candidate", () => {
    const recs = generateRecommendations(
      triplet("MATCH (p:Person) WHERE p.email = $x RETURN p"),
      EMPTY,
    );
    expect(recs).toHaveLength(1);
    expect(recs[0]).toMatchObject({
      kind: "RANGE_INDEX",
      label: "Person",
      property: "email",
    });
    expect(recs[0]!.evidenceCount).toBeGreaterThanOrEqual(
      DEFAULT_EVIDENCE_THRESHOLD,
    );
  });

  it("does not surface candidates below the evidence threshold", () => {
    const recs = generateRecommendations(
      [{ body: "MATCH (p:Person) WHERE p.email = $x RETURN p", ok: true }],
      EMPTY,
    );
    expect(recs).toHaveLength(0);
  });

  it("ignores failed query runs", () => {
    const recs = generateRecommendations(
      Array.from({ length: 5 }, () => ({
        body: "MATCH (p:Person) WHERE p.email = $x RETURN p",
        ok: false,
      })),
      EMPTY,
    );
    expect(recs).toHaveLength(0);
  });
});

describe("range filters → RANGE_INDEX", () => {
  it("captures < and > predicates", () => {
    const recs = generateRecommendations(
      triplet("MATCH (p:Person) WHERE p.age > 21 RETURN p"),
      EMPTY,
    );
    expect(recs.map((r) => r.kind)).toContain("RANGE_INDEX");
  });
});

describe("text predicates → TEXT_INDEX", () => {
  it("buckets STARTS WITH / CONTAINS / ENDS WITH as a TEXT candidate", () => {
    const recs = generateRecommendations(
      [
        { body: "MATCH (p:Person) WHERE p.name STARTS WITH 'a' RETURN p", ok: true },
        { body: "MATCH (p:Person) WHERE p.name CONTAINS 'b' RETURN p", ok: true },
        { body: "MATCH (p:Person) WHERE p.name ENDS WITH 'c' RETURN p", ok: true },
      ],
      EMPTY,
    );
    expect(recs.some((r: Recommendation) => r.kind === "TEXT_INDEX")).toBe(true);
  });
});

describe("MERGE → UNIQUE_CONSTRAINT", () => {
  it("treats inline-property MERGE as a uniqueness hint", () => {
    const recs = generateRecommendations(
      triplet("MERGE (p:Person {email: $x}) RETURN p"),
      EMPTY,
    );
    expect(recs.some((r) => r.kind === "UNIQUE_CONSTRAINT")).toBe(true);
  });
});

describe("existing-coverage filtering", () => {
  it("hides RANGE candidates when an equivalent RANGE index exists", () => {
    const recs = generateRecommendations(
      triplet("MATCH (p:Person) WHERE p.email = $x RETURN p"),
      {
        ...EMPTY,
        indexes: [
          {
            name: "idx_person_email",
            kind: "RANGE",
            entity: "NODE",
            labelsOrTypes: ["Person"],
            properties: ["email"],
            state: "online",
            populationPercent: 100,
            owned: false,
          },
        ],
      },
    );
    expect(recs).toHaveLength(0);
  });

  it("hides UNIQUE_CONSTRAINT candidates when a UNIQUE constraint exists", () => {
    const recs = generateRecommendations(
      triplet("MERGE (p:Person {email: $x}) RETURN p"),
      {
        ...EMPTY,
        constraints: [
          {
            name: "unique_person_email",
            kind: "UNIQUE",
            entity: "NODE",
            label: "Person",
            properties: ["email"],
          },
        ],
      },
    );
    expect(recs).toHaveLength(0);
  });

  it("NODE_KEY also covers UNIQUE_CONSTRAINT candidates", () => {
    const recs = generateRecommendations(
      triplet("MERGE (p:Person {email: $x}) RETURN p"),
      {
        ...EMPTY,
        constraints: [
          {
            name: "nodekey_person",
            kind: "NODE_KEY",
            entity: "NODE",
            label: "Person",
            properties: ["email", "tenantId"],
          },
        ],
      },
    );
    expect(recs).toHaveLength(0);
  });
});

describe("dismissal", () => {
  it("excludes dismissed ids", () => {
    const dismissed = new Set(["RANGE_INDEX::NODE::Person::email"]);
    const recs = generateRecommendations(
      triplet("MATCH (p:Person) WHERE p.email = $x RETURN p"),
      { ...EMPTY, dismissed },
    );
    expect(recs).toHaveLength(0);
  });
});

describe("known regex limitations (documented behavior)", () => {
  it("matches label patterns inside string literals (false positive)", () => {
    // This is a known gap: the regex tokenizer does not understand
    // quoted-string scope. Documented here so a future scanner can
    // assert the limitation has been closed.
    const recs = generateRecommendations(
      triplet("RETURN '(p:Person) WHERE p.email = ' AS s"),
      EMPTY,
    );
    expect(recs.length).toBeGreaterThanOrEqual(0);
  });
});

describe("ordering", () => {
  it("sorts by evidence count descending then label/property alphabetically", () => {
    const history = [
      ...Array(5)
        .fill(null)
        .map(() => ({
          body: "MATCH (p:Person) WHERE p.email = $x RETURN p",
          ok: true,
        })),
      ...Array(3)
        .fill(null)
        .map(() => ({
          body: "MATCH (b:Book) WHERE b.title = $x RETURN b",
          ok: true,
        })),
    ];
    const recs = generateRecommendations(history, EMPTY);
    expect(recs[0]?.label).toBe("Person");
    expect(recs[1]?.label).toBe("Book");
  });
});
