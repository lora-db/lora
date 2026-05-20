/**
 * Snapshot the DDL we emit for every wizard configuration so the
 * generated Cypher stays stable across refactors.
 */

import { describe, expect, it } from "vitest";

import {
  buildCreateConstraintDDL,
  buildCreateIndexDDL,
  buildDropConstraintDDL,
  buildDropIndexDDL,
  constraintDefToDraft,
  indexDefToDraft,
  suggestConstraintName,
  suggestIndexName,
} from "@/lib/schemaDesign/ddl";
import type {
  ConstraintDef,
  ConstraintDraft,
  IndexDef,
  IndexDraft,
} from "@/lib/schemaDesign/types";

const baseIndex: IndexDraft = {
  kind: "RANGE",
  entity: "NODE",
  label: "Person",
  properties: ["email"],
  name: "idx_person_email",
  ifNotExists: true,
};

describe("buildCreateIndexDDL", () => {
  it("emits a RANGE INDEX for a node with one property", () => {
    expect(buildCreateIndexDDL(baseIndex)).toBe(
      "CREATE RANGE INDEX `idx_person_email` IF NOT EXISTS FOR (n:`Person`) ON (n.`email`)",
    );
  });

  it("emits a composite property list", () => {
    expect(
      buildCreateIndexDDL({ ...baseIndex, properties: ["first", "last"] }),
    ).toBe(
      "CREATE RANGE INDEX `idx_person_email` IF NOT EXISTS FOR (n:`Person`) ON (n.`first`, n.`last`)",
    );
  });

  it("emits a TEXT INDEX", () => {
    expect(buildCreateIndexDDL({ ...baseIndex, kind: "TEXT" })).toBe(
      "CREATE TEXT INDEX `idx_person_email` IF NOT EXISTS FOR (n:`Person`) ON (n.`email`)",
    );
  });

  it("emits a FULLTEXT INDEX with ON EACH bracket list", () => {
    expect(
      buildCreateIndexDDL({
        ...baseIndex,
        kind: "FULLTEXT",
        properties: ["title", "body"],
      }),
    ).toBe(
      "CREATE FULLTEXT INDEX `idx_person_email` IF NOT EXISTS FOR (n:`Person`) ON EACH [n.`title`, n.`body`]",
    );
  });

  it("emits a relationship index with type pattern", () => {
    expect(
      buildCreateIndexDDL({
        ...baseIndex,
        entity: "RELATIONSHIP",
        label: "KNOWS",
        properties: ["since"],
      }),
    ).toBe(
      "CREATE RANGE INDEX `idx_person_email` IF NOT EXISTS FOR ()-[r:`KNOWS`]-() ON (r.`since`)",
    );
  });

  it("emits LOOKUP for nodes with EACH labels(n)", () => {
    expect(
      buildCreateIndexDDL({
        ...baseIndex,
        kind: "LOOKUP",
        properties: [],
        label: "",
      }),
    ).toBe(
      "CREATE LOOKUP INDEX `idx_person_email` IF NOT EXISTS FOR (n) ON EACH labels(n)",
    );
  });

  it("emits LOOKUP for relationships with EACH type(r)", () => {
    expect(
      buildCreateIndexDDL({
        ...baseIndex,
        kind: "LOOKUP",
        entity: "RELATIONSHIP",
        properties: [],
        label: "",
      }),
    ).toBe(
      "CREATE LOOKUP INDEX `idx_person_email` IF NOT EXISTS FOR ()-[r]-() ON EACH type(r)",
    );
  });

  it("omits IF NOT EXISTS when disabled", () => {
    expect(buildCreateIndexDDL({ ...baseIndex, ifNotExists: false })).toBe(
      "CREATE RANGE INDEX `idx_person_email` FOR (n:`Person`) ON (n.`email`)",
    );
  });

  it("strips embedded backticks from identifiers", () => {
    expect(
      buildCreateIndexDDL({ ...baseIndex, label: "Bad`Label", properties: ["a`b"] }),
    ).toBe(
      "CREATE RANGE INDEX `idx_person_email` IF NOT EXISTS FOR (n:`BadLabel`) ON (n.`ab`)",
    );
  });
});

describe("buildDropIndexDDL", () => {
  it("includes IF EXISTS by default", () => {
    expect(buildDropIndexDDL("foo")).toBe("DROP INDEX `foo` IF EXISTS");
  });

  it("omits IF EXISTS when asked", () => {
    expect(buildDropIndexDDL("foo", false)).toBe("DROP INDEX `foo`");
  });
});

const baseConstraint: ConstraintDraft = {
  kind: "UNIQUE",
  entity: "NODE",
  label: "Person",
  properties: ["email"],
  propertyType: "STRING",
  name: "unique_person_email",
  ifNotExists: true,
};

describe("buildCreateConstraintDDL", () => {
  it("emits a single-property UNIQUE inside parens", () => {
    expect(buildCreateConstraintDDL(baseConstraint)).toBe(
      "CREATE CONSTRAINT `unique_person_email` IF NOT EXISTS FOR (n:`Person`) REQUIRE (n.`email`) IS UNIQUE",
    );
  });

  it("emits a composite NODE KEY", () => {
    expect(
      buildCreateConstraintDDL({
        ...baseConstraint,
        kind: "NODE_KEY",
        properties: ["country", "taxId"],
        name: "nodekey_person_country_taxid",
      }),
    ).toBe(
      "CREATE CONSTRAINT `nodekey_person_country_taxid` IF NOT EXISTS FOR (n:`Person`) REQUIRE (n.`country`, n.`taxId`) IS NODE KEY",
    );
  });

  it("emits NOT NULL without parens", () => {
    expect(
      buildCreateConstraintDDL({
        ...baseConstraint,
        kind: "NOT_NULL",
        name: "notnull_person_email",
      }),
    ).toBe(
      "CREATE CONSTRAINT `notnull_person_email` IF NOT EXISTS FOR (n:`Person`) REQUIRE n.`email` IS NOT NULL",
    );
  });

  it("emits PROPERTY_TYPE with the type predicate", () => {
    expect(
      buildCreateConstraintDDL({
        ...baseConstraint,
        kind: "PROPERTY_TYPE",
        propertyType: "INTEGER",
        name: "ptype_person_email",
      }),
    ).toBe(
      "CREATE CONSTRAINT `ptype_person_email` IF NOT EXISTS FOR (n:`Person`) REQUIRE n.`email` IS :: INTEGER",
    );
  });

  it("emits RELATIONSHIP_KEY against the rel pattern", () => {
    expect(
      buildCreateConstraintDDL({
        ...baseConstraint,
        kind: "RELATIONSHIP_KEY",
        entity: "RELATIONSHIP",
        label: "FOLLOWS",
        properties: ["from", "to"],
        name: "relkey_follows",
      }),
    ).toBe(
      "CREATE CONSTRAINT `relkey_follows` IF NOT EXISTS FOR ()-[r:`FOLLOWS`]-() REQUIRE (r.`from`, r.`to`) IS RELATIONSHIP KEY",
    );
  });
});

describe("buildDropConstraintDDL", () => {
  it("includes IF EXISTS by default", () => {
    expect(buildDropConstraintDDL("foo")).toBe("DROP CONSTRAINT `foo` IF EXISTS");
  });
});

describe("suggestIndexName", () => {
  it("uses idx_<label>_<props>", () => {
    expect(
      suggestIndexName({
        kind: "RANGE",
        entity: "NODE",
        label: "Person",
        properties: ["email"],
      }),
    ).toBe("idx_person_email");
  });

  it("uses lookup_labels for LOOKUP nodes", () => {
    expect(
      suggestIndexName({
        kind: "LOOKUP",
        entity: "NODE",
        label: "",
        properties: [],
      }),
    ).toBe("lookup_labels");
  });

  it("falls back to label-only when no props", () => {
    expect(
      suggestIndexName({
        kind: "RANGE",
        entity: "NODE",
        label: "Person",
        properties: [],
      }),
    ).toBe("idx_person");
  });
});

describe("suggestConstraintName", () => {
  it("uses unique_<label>_<prop>", () => {
    expect(
      suggestConstraintName({
        kind: "UNIQUE",
        label: "Person",
        properties: ["email"],
      }),
    ).toBe("unique_person_email");
  });

  it("uses notnull_ prefix", () => {
    expect(
      suggestConstraintName({
        kind: "NOT_NULL",
        label: "Person",
        properties: ["email"],
      }),
    ).toBe("notnull_person_email");
  });
});

describe("indexDefToDraft", () => {
  const baseDef: IndexDef = {
    name: "idx_person_email",
    kind: "RANGE",
    entity: "NODE",
    labelsOrTypes: ["Person"],
    properties: ["email"],
    state: "online",
    populationPercent: 100,
    owned: false,
  };

  it("lifts the def's shape into a draft and round-trips through the builder", () => {
    const draft = indexDefToDraft(baseDef);
    expect(draft).toEqual({
      kind: "RANGE",
      entity: "NODE",
      label: "Person",
      properties: ["email"],
      name: "idx_person_email",
      ifNotExists: false,
    });
    expect(buildCreateIndexDDL(draft)).toBe(
      "CREATE RANGE INDEX `idx_person_email` FOR (n:`Person`) ON (n.`email`)",
    );
  });

  it("handles LOOKUP defs whose labelsOrTypes is empty", () => {
    const draft = indexDefToDraft({
      ...baseDef,
      kind: "LOOKUP",
      labelsOrTypes: [],
      properties: [],
    });
    expect(draft.label).toBe("");
  });

  it("clones the properties array so mutations don't bleed back", () => {
    const draft = indexDefToDraft(baseDef);
    draft.properties.push("extra");
    expect(baseDef.properties).toEqual(["email"]);
  });
});

describe("constraintDefToDraft", () => {
  const baseDef: ConstraintDef = {
    name: "unique_person_email",
    kind: "UNIQUE",
    entity: "NODE",
    label: "Person",
    properties: ["email"],
  };

  it("lifts the def's shape into a draft", () => {
    const draft = constraintDefToDraft(baseDef);
    expect(draft).toEqual({
      kind: "UNIQUE",
      entity: "NODE",
      label: "Person",
      properties: ["email"],
      propertyType: "STRING",
      name: "unique_person_email",
      ifNotExists: false,
    });
  });

  it("falls back to STRING when the def carries a non-scalar property type", () => {
    const draft = constraintDefToDraft({
      ...baseDef,
      kind: "PROPERTY_TYPE",
      propertyType: "LIST<INTEGER>",
    });
    expect(draft.propertyType).toBe("STRING");
  });

  it("preserves a recognised scalar propertyType verbatim", () => {
    const draft = constraintDefToDraft({
      ...baseDef,
      kind: "PROPERTY_TYPE",
      propertyType: "INTEGER",
    });
    expect(draft.propertyType).toBe("INTEGER");
  });
});
