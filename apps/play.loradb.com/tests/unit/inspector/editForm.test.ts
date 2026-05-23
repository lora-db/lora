/**
 * Pure-function tests for the inspector's edit-mode form helpers —
 * row construction, value parsing, cross-row validation, and the
 * dirty-state check used to gate "discard changes?" confirmations.
 */

import { describe, expect, it } from "vitest";

import {
  buildPropertiesPayload,
  editableKindFor,
  parseRow,
  rowFromValue,
  rowsDirty,
  rowsFromProperties,
  validateKey,
  validateRows,
  type EditRow,
} from "@/app/_components/Inspector/editForm";

describe("editableKindFor", () => {
  it("maps detected kinds to editor kinds", () => {
    expect(editableKindFor("string")).toBe("string");
    expect(editableKindFor("url")).toBe("string");
    expect(editableKindFor("email")).toBe("string");
    expect(editableKindFor("datetime")).toBe("string");
    expect(editableKindFor("integer")).toBe("integer");
    expect(editableKindFor("float")).toBe("float");
    expect(editableKindFor("boolean")).toBe("boolean");
    expect(editableKindFor("null")).toBe("null");
    expect(editableKindFor("array")).toBe("json");
    expect(editableKindFor("object")).toBe("json");
    expect(editableKindFor("point")).toBe("json");
    expect(editableKindFor("duration")).toBe("json");
    expect(editableKindFor("bigint")).toBe("json");
  });
});

describe("rowFromValue", () => {
  it("renders a string value with its text form", () => {
    const row = rowFromValue("name", "Alice");
    expect(row.key).toBe("name");
    expect(row.kind).toBe("string");
    expect(row.text).toBe("Alice");
  });

  it("renders a boolean as a toggle", () => {
    const row = rowFromValue("active", true);
    expect(row.kind).toBe("boolean");
    expect(row.bool).toBe(true);
  });

  it("renders an integer with the canonical text form", () => {
    const row = rowFromValue("age", 42);
    expect(row.kind).toBe("integer");
    expect(row.text).toBe("42");
  });

  it("renders arrays and objects as JSON", () => {
    expect(rowFromValue("tags", ["a", "b"]).kind).toBe("json");
    expect(rowFromValue("tags", ["a", "b"]).text).toContain("[");
    expect(rowFromValue("meta", { foo: 1 }).kind).toBe("json");
  });

  it("tags engine shapes as JSON with a raw-kind hint", () => {
    const row = rowFromValue("home", { srid: 4326, x: 1, y: 2 });
    expect(row.kind).toBe("json");
    expect(row.rawHint).toBe("point");
  });

  it("handles null by producing a null row", () => {
    const row = rowFromValue("missing", null);
    expect(row.kind).toBe("null");
  });
});

describe("rowsFromProperties", () => {
  it("preserves insertion order", () => {
    const rows = rowsFromProperties({ b: 1, a: "x", c: true });
    expect(rows.map((r) => r.key)).toEqual(["b", "a", "c"]);
  });
});

describe("validateKey", () => {
  it("rejects empty / whitespace keys", () => {
    expect(validateKey("")).toMatch(/required/i);
    expect(validateKey("  ")).toMatch(/required/i);
  });

  it("rejects keys with surrounding whitespace", () => {
    expect(validateKey(" name ")).toMatch(/spaces/i);
  });

  it("rejects dotted keys", () => {
    expect(validateKey("user.name")).toMatch(/'\.'/);
  });

  it("accepts normal identifiers", () => {
    expect(validateKey("name")).toBeNull();
    expect(validateKey("user_id")).toBeNull();
    expect(validateKey("camelCase")).toBeNull();
  });
});

describe("parseRow", () => {
  it("parses booleans straight from the toggle", () => {
    const row: EditRow = {
      uid: "1",
      key: "x",
      kind: "boolean",
      text: "",
      bool: true,
    };
    expect(parseRow(row)).toEqual({ ok: true, value: true });
  });

  it("rejects bad integers but allows empty (null)", () => {
    const baseI: EditRow = {
      uid: "i",
      key: "n",
      kind: "integer",
      text: "abc",
      bool: false,
    };
    expect(parseRow(baseI).ok).toBe(false);
    expect(parseRow({ ...baseI, text: "12.5" }).ok).toBe(false);
    expect(parseRow({ ...baseI, text: "" })).toEqual({ ok: true, value: null });
    expect(parseRow({ ...baseI, text: "-7" })).toEqual({ ok: true, value: -7 });
  });

  it("parses floats permissively", () => {
    const base: EditRow = {
      uid: "f",
      key: "n",
      kind: "float",
      text: "3.14",
      bool: false,
    };
    expect(parseRow(base)).toEqual({ ok: true, value: 3.14 });
    expect(parseRow({ ...base, text: "1e3" })).toEqual({
      ok: true,
      value: 1000,
    });
    expect(parseRow({ ...base, text: "x" }).ok).toBe(false);
  });

  it("rejects malformed JSON", () => {
    const row: EditRow = {
      uid: "j",
      key: "tags",
      kind: "json",
      text: "[1, 2,",
      bool: false,
    };
    expect(parseRow(row).ok).toBe(false);
  });

  it("returns the parsed JSON shape on success", () => {
    const row: EditRow = {
      uid: "j",
      key: "tags",
      kind: "json",
      text: '["a","b"]',
      bool: false,
    };
    expect(parseRow(row)).toEqual({ ok: true, value: ["a", "b"] });
  });
});

describe("validateRows", () => {
  it("flags duplicate keys on the second occurrence", () => {
    const rows: EditRow[] = [
      { uid: "1", key: "name", kind: "string", text: "Alice", bool: false },
      { uid: "2", key: "name", kind: "string", text: "Bob", bool: false },
    ];
    const result = validateRows(rows);
    expect(result.rowErrors).toEqual([
      { uid: "2", message: 'Duplicate key "name".' },
    ]);
  });

  it("surfaces missing required keys as a global error", () => {
    const rows: EditRow[] = [
      { uid: "1", key: "name", kind: "string", text: "Alice", bool: false },
    ];
    const result = validateRows(rows, { requiredKeys: new Set(["email"]) });
    expect(result.globalError).toContain("email");
  });

  it("marks a present-but-null required key as a row error", () => {
    const rows: EditRow[] = [
      { uid: "1", key: "email", kind: "null", text: "", bool: false },
    ];
    const result = validateRows(rows, { requiredKeys: new Set(["email"]) });
    expect(result.rowErrors).toHaveLength(1);
    expect(result.rowErrors[0]?.message).toMatch(/required/i);
  });

  it("passes a clean form with required keys all set", () => {
    const rows: EditRow[] = [
      { uid: "1", key: "email", kind: "string", text: "a@b.com", bool: false },
      { uid: "2", key: "name", kind: "string", text: "Alice", bool: false },
    ];
    const result = validateRows(rows, { requiredKeys: new Set(["email"]) });
    expect(result.rowErrors).toEqual([]);
    expect(result.globalError).toBeNull();
  });
});

describe("buildPropertiesPayload", () => {
  it("returns null when any row is invalid", () => {
    const rows: EditRow[] = [
      { uid: "1", key: "", kind: "string", text: "x", bool: false },
    ];
    expect(buildPropertiesPayload(rows)).toBeNull();
  });

  it("returns the final payload for valid rows", () => {
    const rows: EditRow[] = [
      { uid: "1", key: "name", kind: "string", text: "Alice", bool: false },
      { uid: "2", key: "age", kind: "integer", text: "42", bool: false },
      { uid: "3", key: "active", kind: "boolean", text: "", bool: true },
      { uid: "4", key: "tags", kind: "json", text: '["a"]', bool: false },
    ];
    expect(buildPropertiesPayload(rows)).toEqual({
      name: "Alice",
      age: 42,
      active: true,
      tags: ["a"],
    });
  });
});

describe("rowsDirty", () => {
  it("returns false when the form matches the original map", () => {
    const original = { name: "Alice", age: 42 };
    const rows = rowsFromProperties(original);
    expect(rowsDirty(rows, original)).toBe(false);
  });

  it("returns true after a value edit", () => {
    const original = { name: "Alice" };
    const rows = rowsFromProperties(original);
    rows[0]!.text = "Bob";
    expect(rowsDirty(rows, original)).toBe(true);
  });

  it("returns true after a property removal", () => {
    const original = { name: "Alice", age: 42 };
    const rows = rowsFromProperties(original).filter((r) => r.key !== "age");
    expect(rowsDirty(rows, original)).toBe(true);
  });

  it("treats an invalid form as dirty so it can't be silently dismissed", () => {
    const original = { name: "Alice" };
    const rows: EditRow[] = [
      { uid: "1", key: "", kind: "string", text: "", bool: false },
    ];
    expect(rowsDirty(rows, original)).toBe(true);
  });
});
