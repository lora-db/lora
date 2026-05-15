import { describe, expect, it } from "vitest";
import { detectQueryFolds, pickFoldForLine } from "../src/cypher/folding";
import type { FoldRange } from "../src/parser";

describe("detectQueryFolds — top-level statements", () => {
  it("single query without trailing `;` becomes one fold range", () => {
    const src = "MATCH (n)\nRETURN n\nLIMIT 10";
    const out = detectQueryFolds(src);
    expect(out).toHaveLength(1);
    expect(out[0]!.kind).toBe("query");
    expect(out[0]!.start).toBe(0);
    expect(out[0]!.end).toBe(src.length);
  });

  it("multiple statements split at top-level `;`", () => {
    const src =
      "MATCH (n) RETURN n;\n\nMATCH (m) RETURN m LIMIT 5;\nMATCH (a) RETURN a";
    const out = detectQueryFolds(src);
    expect(out).toHaveLength(3);
    expect(out.map((r) => r.kind)).toEqual(["query", "query", "query"]);
    // Each range covers a single statement.
    expect(src.slice(out[0]!.start, out[0]!.end)).toBe("MATCH (n) RETURN n");
    expect(src.slice(out[1]!.start, out[1]!.end)).toBe(
      "MATCH (m) RETURN m LIMIT 5",
    );
    expect(src.slice(out[2]!.start, out[2]!.end)).toBe("MATCH (a) RETURN a");
  });

  it("ignores `;` inside strings", () => {
    const src = "MATCH (n) WHERE n.name = 'a;b;c' RETURN n";
    const out = detectQueryFolds(src);
    expect(out).toHaveLength(1);
    expect(src.slice(out[0]!.start, out[0]!.end)).toBe(src);
  });

  it("ignores `;` inside line comments", () => {
    const src = "MATCH (n) // ;;; comment\nRETURN n";
    const out = detectQueryFolds(src);
    expect(out).toHaveLength(1);
  });

  it("ignores `;` inside balanced delimiters", () => {
    const src = "RETURN { a: ';' , b: [1, 2, 3] }";
    const out = detectQueryFolds(src);
    expect(out).toHaveLength(1);
  });

  it("trims leading and trailing whitespace from each range", () => {
    const src = "   MATCH (n) RETURN n   ;   MATCH (m) RETURN m   ";
    const out = detectQueryFolds(src);
    expect(out).toHaveLength(2);
    expect(src.slice(out[0]!.start, out[0]!.end)).toBe("MATCH (n) RETURN n");
    expect(src.slice(out[1]!.start, out[1]!.end)).toBe("MATCH (m) RETURN m");
  });

  it("empty input → no ranges", () => {
    expect(detectQueryFolds("")).toEqual([]);
  });

  it("whitespace-only input → no ranges", () => {
    expect(detectQueryFolds("   \n\n  ")).toEqual([]);
  });
});

describe("pickFoldForLine — chevron picks largest range starting on the line", () => {
  it("returns null when no range starts on the line", () => {
    const ranges: FoldRange[] = [{ start: 50, end: 100, kind: "query" }];
    expect(pickFoldForLine(ranges, 0, 10)).toBeNull();
  });

  it("returns null when ranges starting on the line don't extend past it", () => {
    // A clause fold that lives entirely on this line shouldn't surface
    // a chevron — there's nothing to collapse.
    const ranges: FoldRange[] = [{ start: 0, end: 9, kind: "match" }];
    expect(pickFoldForLine(ranges, 0, 10)).toBeNull();
  });

  it("prefers the LARGEST range when multiple start on the same line", () => {
    // Same first line: query covers the whole 200-byte statement,
    // MATCH clause only the first 40. The chevron should fold the
    // whole query, not just MATCH — otherwise the user can never fold
    // a multi-clause statement from its opening line.
    const ranges: FoldRange[] = [
      { start: 0, end: 40, kind: "match" },
      { start: 0, end: 200, kind: "query" },
    ];
    const result = pickFoldForLine(ranges, 0, 10);
    expect(result).toEqual({ from: 10, to: 200 });
  });

  it("inner clause folds remain reachable on their own starting line", () => {
    // The WHERE clause starts on line 2 (lineStart 30). Even though
    // the query fold also exists in `ranges`, its start (0) does not
    // fall in [30, 50], so the chevron on the WHERE line picks the
    // inner clause fold.
    const ranges: FoldRange[] = [
      { start: 0, end: 200, kind: "query" },
      { start: 30, end: 80, kind: "where" },
    ];
    const result = pickFoldForLine(ranges, 30, 50);
    expect(result).toEqual({ from: 50, to: 80 });
  });

  it("multi-statement: each statement's first line gets its own query fold", () => {
    // Three top-level statements. Each owns one query range. Line for
    // statement 2 begins at offset 30; only statement 2's range
    // matches.
    const ranges: FoldRange[] = [
      { start: 0, end: 25, kind: "query" },
      { start: 30, end: 60, kind: "query" },
      { start: 65, end: 95, kind: "query" },
    ];
    // First line of statement 2 (single-line statement → no fold).
    expect(pickFoldForLine(ranges, 30, 60)).toBeNull();
    // Multi-line statement 2: still one range, but now extends past
    // the first line.
    const ranges2: FoldRange[] = [
      { start: 0, end: 25, kind: "query" },
      { start: 30, end: 90, kind: "query" },
      { start: 95, end: 120, kind: "query" },
    ];
    expect(pickFoldForLine(ranges2, 30, 50)).toEqual({ from: 50, to: 90 });
  });
});
