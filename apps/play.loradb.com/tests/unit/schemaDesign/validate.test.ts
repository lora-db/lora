/**
 * Coverage for client-side draft validation. Mirrors the engine-side
 * checks so the wizard catches mistakes before paying for a
 * roundtrip.
 */

import { describe, expect, it } from "vitest";

import {
  isSubmittable,
  validateConstraintDraft,
  validateIndexDraft,
} from "@/lib/schemaDesign/validate";
import type {
  ConstraintDef,
  ConstraintDraft,
  IndexDef,
  IndexDraft,
} from "@/lib/schemaDesign/types";

const emptyCatalog = { indexes: [] as IndexDef[], constraints: [] as ConstraintDef[] };

const validIndex: IndexDraft = {
  kind: "RANGE",
  entity: "NODE",
  label: "Person",
  properties: ["email"],
  name: "idx_person_email",
  ifNotExists: true,
};

describe("validateIndexDraft", () => {
  it("accepts a complete draft", () => {
    expect(validateIndexDraft(validIndex, emptyCatalog)).toEqual([]);
  });

  it("rejects an invalid name", () => {
    const issues = validateIndexDraft({ ...validIndex, name: "1bad" }, emptyCatalog);
    expect(issues.some((i) => i.field === "name" && i.blocking)).toBe(true);
  });

  it("rejects a name clash with an existing index", () => {
    const existing: IndexDef[] = [
      {
        name: "idx_person_email",
        kind: "RANGE",
        entity: "NODE",
        labelsOrTypes: ["Other"],
        properties: ["x"],
        state: "online",
        populationPercent: 100,
        owned: false,
      },
    ];
    const issues = validateIndexDraft(validIndex, { ...emptyCatalog, indexes: existing });
    expect(issues.some((i) => i.field === "name" && i.blocking)).toBe(true);
  });

  it("rejects an empty property list for non-LOOKUP", () => {
    const issues = validateIndexDraft(
      { ...validIndex, properties: [] },
      emptyCatalog,
    );
    expect(issues.some((i) => i.field === "properties" && i.blocking)).toBe(true);
  });

  it("flags a RANGE conflict with an owned index", () => {
    const owned: IndexDef[] = [
      {
        name: "auto_index",
        kind: "RANGE",
        entity: "NODE",
        labelsOrTypes: ["Person"],
        properties: ["email"],
        state: "online",
        populationPercent: 100,
        owned: true,
        ownerConstraint: "unique_person_email",
      },
    ];
    const issues = validateIndexDraft(
      { ...validIndex, name: "different_name" },
      { ...emptyCatalog, indexes: owned },
    );
    expect(issues.some((i) => i.field === "kind")).toBe(true);
  });

  it("LOOKUP indexes accept empty properties + label", () => {
    const issues = validateIndexDraft(
      {
        ...validIndex,
        kind: "LOOKUP",
        label: "",
        properties: [],
        name: "lookup_labels",
      },
      emptyCatalog,
    );
    expect(isSubmittable(issues)).toBe(true);
  });
});

const validConstraint: ConstraintDraft = {
  kind: "UNIQUE",
  entity: "NODE",
  label: "Person",
  properties: ["email"],
  propertyType: "STRING",
  name: "unique_person_email",
  ifNotExists: true,
};

describe("validateConstraintDraft", () => {
  it("accepts a complete draft", () => {
    expect(validateConstraintDraft(validConstraint, emptyCatalog)).toEqual([]);
  });

  it("rejects NODE_KEY on relationship entity", () => {
    const issues = validateConstraintDraft(
      { ...validConstraint, kind: "NODE_KEY", entity: "RELATIONSHIP", properties: ["a", "b"] },
      emptyCatalog,
    );
    expect(issues.some((i) => i.field === "kind")).toBe(true);
  });

  it("requires 2+ properties for NODE_KEY", () => {
    const issues = validateConstraintDraft(
      { ...validConstraint, kind: "NODE_KEY", properties: ["only"] },
      emptyCatalog,
    );
    expect(issues.some((i) => i.field === "properties" && i.blocking)).toBe(true);
  });

  it("blocks NOT_NULL with multiple properties", () => {
    const issues = validateConstraintDraft(
      { ...validConstraint, kind: "NOT_NULL", properties: ["a", "b"] },
      emptyCatalog,
    );
    expect(issues.some((i) => i.field === "properties" && i.blocking)).toBe(true);
  });

  it("flags an exact duplicate", () => {
    const existing: ConstraintDef[] = [
      {
        name: "other_name",
        kind: "UNIQUE",
        entity: "NODE",
        label: "Person",
        properties: ["email"],
      },
    ];
    const issues = validateConstraintDraft(validConstraint, {
      ...emptyCatalog,
      constraints: existing,
    });
    expect(issues.some((i) => i.field === "kind" && i.blocking)).toBe(true);
  });
});
