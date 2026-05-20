import { describe, expect, it } from "vitest";

import {
  labelCount,
  labelDistinctProperty,
  labelMatch,
  labelNeighbors,
  labelSample,
  propertyDistinctAny,
  quoteId,
  relTypeCount,
  relTypeDistinctProperty,
  relTypeEndpoints,
  relTypeMatch,
} from "@/lib/snippets/cypher";

describe("snippets/cypher — quoteId", () => {
  it("leaves plain identifiers unquoted", () => {
    expect(quoteId("Venue")).toBe("Venue");
    expect(quoteId("osm_id")).toBe("osm_id");
    expect(quoteId("_private")).toBe("_private");
  });

  it("wraps identifiers that need escaping", () => {
    expect(quoteId("has space")).toBe("`has space`");
    expect(quoteId("9starts-digit")).toBe("`9starts-digit`");
    expect(quoteId("dash-name")).toBe("`dash-name`");
  });

  it("strips embedded backticks defensively when quoting", () => {
    expect(quoteId("we`ird")).toBe("`weird`");
  });
});

describe("snippets/cypher — labels", () => {
  it("emits a MATCH/RETURN with LIMIT for plain labels", () => {
    expect(labelMatch("Venue")).toBe("MATCH (v:Venue)\nRETURN v\nLIMIT 25");
  });

  it("projects a property when one is requested", () => {
    expect(labelMatch("Venue", { property: "osm_id" })).toBe(
      "MATCH (v:Venue)\nRETURN v.osm_id\nLIMIT 25",
    );
  });

  it("backticks labels that need escaping", () => {
    expect(labelMatch("has space")).toBe(
      "MATCH (h:`has space`)\nRETURN h\nLIMIT 25",
    );
  });

  it("falls back to `n` when the binding letter would be empty", () => {
    // Non-alphanumeric labels strip down to empty; the binding name
    // must still be a valid Cypher identifier.
    expect(labelMatch("!@#")).toContain("MATCH (n:`!@#`)");
  });

  it("emits a count query", () => {
    expect(labelCount("Venue")).toBe(
      "MATCH (v:Venue)\nRETURN count(v) AS count",
    );
  });

  it("emits a single-row sample", () => {
    expect(labelSample("Venue")).toBe("MATCH (v:Venue)\nRETURN v\nLIMIT 1");
  });

  it("emits a distinct-property query", () => {
    expect(labelDistinctProperty("Venue", "category")).toBe(
      "MATCH (v:Venue)\nRETURN DISTINCT v.category AS category\nORDER BY category\nLIMIT 25",
    );
  });

  it("emits a neighbors query", () => {
    expect(labelNeighbors("Venue")).toBe(
      "MATCH (v:Venue)-[r]-(m)\nRETURN v, r, m\nLIMIT 25",
    );
  });
});

describe("snippets/cypher — rel-types", () => {
  it("emits a MATCH/RETURN for rel-types", () => {
    expect(relTypeMatch("VISITED")).toBe(
      "MATCH ()-[r:VISITED]->()\nRETURN r\nLIMIT 25",
    );
  });

  it("emits a count over relationships", () => {
    expect(relTypeCount("VISITED")).toBe(
      "MATCH ()-[r:VISITED]->()\nRETURN count(r) AS count",
    );
  });

  it("emits endpoints query", () => {
    expect(relTypeEndpoints("VISITED")).toBe(
      "MATCH (a)-[r:VISITED]->(b)\nRETURN a, r, b\nLIMIT 25",
    );
  });

  it("emits a distinct-property query for rel-types", () => {
    expect(relTypeDistinctProperty("VISITED", "since")).toBe(
      "MATCH ()-[r:VISITED]->()\nRETURN DISTINCT r.since AS since\nORDER BY since\nLIMIT 25",
    );
  });
});

describe("snippets/cypher — flat property keys", () => {
  it("emits a distinct query that ignores nulls", () => {
    expect(propertyDistinctAny("name")).toBe(
      "MATCH (n)\nWHERE n.name IS NOT NULL\nRETURN DISTINCT n.name AS name\nORDER BY name\nLIMIT 25",
    );
  });
});
