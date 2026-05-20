/**
 * Pure-function tests for the inspector's property kind detection,
 * title heuristic, and semantic grouping. These run without React.
 */

import { describe, expect, it } from "vitest";

import {
  detectPropertyKind,
  pickTitleProperty,
  renderValueText,
  semanticGroupFor,
} from "@/app/_components/Inspector/propertyValue";

describe("detectPropertyKind", () => {
  it("classifies primitives", () => {
    expect(detectPropertyKind(null)).toBe("null");
    expect(detectPropertyKind(undefined)).toBe("null");
    expect(detectPropertyKind(true)).toBe("boolean");
    expect(detectPropertyKind(false)).toBe("boolean");
    expect(detectPropertyKind(42)).toBe("integer");
    expect(detectPropertyKind(3.14)).toBe("float");
    expect(detectPropertyKind(1n)).toBe("bigint");
  });

  it("detects urls and emails before falling back to string", () => {
    expect(detectPropertyKind("https://example.com")).toBe("url");
    expect(detectPropertyKind("http://example.com/path")).toBe("url");
    expect(detectPropertyKind("www.example.com")).toBe("url");
    expect(detectPropertyKind("foo@bar.com")).toBe("email");
    expect(detectPropertyKind("hello world")).toBe("string");
  });

  it("detects ISO date-times", () => {
    expect(detectPropertyKind("2026-01-01")).toBe("datetime");
    expect(detectPropertyKind("2026-01-01T12:00:00Z")).toBe("datetime");
    expect(detectPropertyKind("2026-01-01T12:00:00+02:00")).toBe("datetime");
    expect(detectPropertyKind("2026-13-01")).toBe("datetime"); // regex is permissive
    expect(detectPropertyKind("not a date")).toBe("string");
  });

  it("detects neo4j-shaped point and duration objects", () => {
    expect(detectPropertyKind({ srid: 4326, x: 1, y: 2 })).toBe("point");
    expect(detectPropertyKind({ srid: 4326, x: 1, y: 2, z: 3 })).toBe("point");
    expect(detectPropertyKind({ months: 0, days: 1, seconds: 3600 })).toBe(
      "duration",
    );
    expect(detectPropertyKind({ months: 1, days: 0, seconds: 0 })).toBe(
      "duration",
    );
  });

  it("handles arrays and plain objects", () => {
    expect(detectPropertyKind([1, 2, 3])).toBe("array");
    expect(detectPropertyKind({ foo: "bar" })).toBe("object");
  });
});

describe("pickTitleProperty", () => {
  it("prefers name over other keys", () => {
    expect(
      pickTitleProperty({ name: "Alice", email: "a@b.com", title: "x" }),
    ).toBe("name");
  });

  it("falls back to title, then label, then email", () => {
    expect(pickTitleProperty({ title: "Director", id: 1 })).toBe("title");
    expect(pickTitleProperty({ label: "Important" })).toBe("label");
    expect(pickTitleProperty({ email: "a@b.com" })).toBe("email");
  });

  it("returns null when no hint key carries a non-empty string", () => {
    expect(pickTitleProperty({})).toBeNull();
    expect(pickTitleProperty({ name: "  " })).toBeNull(); // whitespace-only trims to empty
    expect(pickTitleProperty({ name: "" })).toBeNull();
    expect(pickTitleProperty({ name: 42 })).toBeNull();
  });
});

describe("semanticGroupFor", () => {
  const constrained = new Set<string>(["email"]);

  it("treats constrained keys as identifiers", () => {
    expect(semanticGroupFor("email", "a@b.com", constrained)).toBe(
      "identifiers",
    );
  });

  it("treats id-like keys as identifiers", () => {
    expect(semanticGroupFor("id", 1, new Set())).toBe("identifiers");
    expect(semanticGroupFor("user_id", "u_1", new Set())).toBe("identifiers");
    expect(semanticGroupFor("uuid", "abc", new Set())).toBe("identifiers");
  });

  it("buckets temporal, spatial, descriptor values correctly", () => {
    expect(semanticGroupFor("createdAt", "2026-01-01", new Set())).toBe(
      "temporal",
    );
    expect(
      semanticGroupFor("home", { srid: 4326, x: 1, y: 2 }, new Set()),
    ).toBe("spatial");
    expect(semanticGroupFor("name", "Alice", new Set())).toBe("descriptors");
    expect(semanticGroupFor("description", "x", new Set())).toBe("descriptors");
  });

  it("falls back to other", () => {
    expect(semanticGroupFor("score", 42, new Set())).toBe("other");
  });
});

describe("renderValueText", () => {
  it("renders primitives, points, durations, arrays", () => {
    expect(renderValueText(null)).toBe("null");
    expect(renderValueText(true)).toBe("true");
    expect(renderValueText(42)).toBe("42");
    expect(renderValueText("hi")).toBe("hi");
    expect(renderValueText({ srid: 4326, x: 1, y: 2 })).toBe("POINT(1 2)");
    expect(renderValueText({ srid: 4326, x: 1, y: 2, z: 3 })).toBe(
      "POINT(1 2 3)",
    );
    expect(renderValueText({ months: 0, days: 1, seconds: 3600 })).toBe(
      "1d 1h",
    );
    expect(renderValueText([1, 2, 3])).toContain("[\n");
  });
});
