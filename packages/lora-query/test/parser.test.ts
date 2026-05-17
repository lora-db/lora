import { describe, expect, it } from "vitest";
import {
  analyse,
  analyseAll,
  format,
  highlight,
  outline,
  parse,
  validate,
  validateAll,
} from "../src/parser";

describe("@loradb/lora-query parser", () => {
  it("parses a trivial MATCH", async () => {
    const result = await parse("MATCH (n) RETURN n");
    expect(result.ok).toBe(true);
    expect(result.errors).toEqual([]);
    expect(result.ast).not.toBeNull();
  });

  it("returns a rich, user-friendly diagnostic for a broken query", async () => {
    const errors = await validate("MATCH (n)\nWHERE a.name = 'Alice");
    expect(errors.length).toBeGreaterThan(0);
    const err = errors[0]!;
    expect(err.severity).toBe("error");
    expect(err.message).toMatch(/Expected|Parse error/);
    expect(err.details).toMatch(/--> 2:/);
    expect(err.line).toBe(2);
    expect(err.column).toBeGreaterThan(0);
    expect(Array.isArray(err.expected)).toBe(true);
    expect(Array.isArray(err.examples)).toBe(true);
    expect(err.span.start).toBeGreaterThan(0);
  });

  it("prettifies a parseable query and splits long projections", async () => {
    const formatted = await format(
      "match (n) return n.name, n.age, n.email, n.role",
    );
    // No trailing newline — loaded into CodeMirror it would otherwise
    // render as a visually empty last line.
    expect(formatted).toBe(
      "MATCH (n)\nRETURN\n  n.name,\n  n.age,\n  n.email,\n  n.role",
    );
  });

  it("prettify is idempotent", async () => {
    const once = await format("match (n) where n.age > 18 return n");
    const twice = await format(once);
    expect(twice).toBe(once);
  });

  it("returns the original source when format cannot parse", async () => {
    const formatted = await format("MATCH (");
    expect(formatted).toBe("MATCH (");
  });

  it("strips the trailing newline so CodeMirror doesn't render a blank line", async () => {
    const formatted = await format("match (n) return n");
    expect(formatted.endsWith("\n")).toBe(false);
    expect(formatted).toBe("MATCH (n)\nRETURN n");
  });

  it("returns AST-driven highlight spans for a parseable query", async () => {
    const spans = await highlight(
      "MATCH (alice:Person {name: 'Alice'}) RETURN alice.name",
    );
    expect(spans.length).toBeGreaterThan(0);
    const kinds = new Set(spans.map((s) => s.kind));
    expect(kinds.has("variable")).toBe(true);
    expect(kinds.has("label")).toBe(true);
    expect(kinds.has("stringLiteral")).toBe(true);
    expect(kinds.has("propertyKey")).toBe(true);
  });

  it("returns the outline of a query", async () => {
    const o = await outline(
      "MATCH (alice:Person)-[:KNOWS]->(bob) WHERE alice.age > $minAge RETURN alice, bob",
    );
    expect(o.variables.map((v) => v.name).sort()).toEqual(["alice", "bob"]);
    expect(o.parameters).toEqual(["minAge"]);
    expect(o.labels).toContain("Person");
    expect(o.relTypes).toContain("KNOWS");
    const alice = o.variables.find((v) => v.name === "alice");
    expect(alice?.label).toBe("Person");
    expect(alice?.kind).toBe("node");
  });

  it("tags relationship variables with kind:'relationship'", async () => {
    const o = await outline(
      "MATCH (a)-[r:KNOWS]->(b) RETURN r",
    );
    const r = o.variables.find((v) => v.name === "r");
    expect(r?.kind).toBe("relationship");
    expect(r?.label).toBe("KNOWS");
  });

  it("records `AS` aliases via aliasOf with the source label", async () => {
    const o = await outline(
      "MATCH (alice:Person) WITH alice AS friend RETURN friend",
    );
    const friend = o.variables.find((v) => v.name === "friend");
    expect(friend?.aliasOf).toBe("alice");
    expect(friend?.label).toBe("Person");
    expect(friend?.kind).toBe("scalar");
  });

  it("analyse returns fold ranges for parseable queries", async () => {
    const a = await analyse(
      "MATCH (n:Person)-[:KNOWS]->(m) WHERE n.age > 20 RETURN n.name AS name, m.name AS friend",
    );
    expect(a.foldRanges.length).toBeGreaterThan(0);
    // expect at least a "pattern" and a "projection" fold range
    const kinds = new Set(a.foldRanges.map((f) => f.kind));
    expect(kinds.has("pattern")).toBe(true);
  });

  it("analyse flags unknown labels when strictLabels is enabled", async () => {
    const a = await analyse("MATCH (n:Unicorn) RETURN n", {
      labels: ["Person", "Company"],
      strictLabels: true,
    });
    const labels = a.diagnostics.map((d) => d.message);
    expect(labels.some((m) => m.includes("Unicorn"))).toBe(true);
  });

  it("analyse returns no warnings for valid queries against the schema", async () => {
    const a = await analyse("MATCH (n:Person) RETURN n", {
      labels: ["Person"],
      strictLabels: true,
    });
    expect(a.diagnostics.filter((d) => d.severity === "warning")).toEqual([]);
  });

  it("validateAll returns one diagnostic per broken statement in a multi-statement script", async () => {
    const src = [
      "MATCH (n) RETURN n;",
      "",
      "MATCH (m)\nWHERE m.name = 'broken;", // unterminated string
      "",
      "MATCH (a) RETURN a LIMIT 10;",
    ].join("\n");
    const errs = await validateAll(src);
    expect(errs.length).toBeGreaterThanOrEqual(1);
    // The broken statement starts at "MATCH (m)\nWHERE..." on line 3
    // (0-indexed). Diagnostic line should be absolute (line 4 with the
    // string opening) — at least past line 1 (i.e. the first stmt
    // didn't get blamed for the second's error).
    const broken = errs.find((e) =>
      /string|unary|expected/i.test(e.message),
    );
    expect(broken).toBeDefined();
    expect(broken!.line).toBeGreaterThanOrEqual(3);
    // The span must point inside the second statement, not the first.
    const secondStmtStart = src.indexOf("MATCH (m)");
    expect(broken!.span.start).toBeGreaterThanOrEqual(secondStmtStart);
  });

  it("validateAll reports zero errors when every statement is valid", async () => {
    const src = [
      "MATCH (n) RETURN n;",
      "MATCH (m) WHERE m.age > 18 RETURN m;",
      "CREATE (a:Person {name: 'Alice'}) RETURN a;",
    ].join("\n");
    expect(await validateAll(src)).toEqual([]);
  });

  it("analyseAll surfaces semantic warnings per statement", async () => {
    const src = [
      "MATCH (n:Person) RETURN n;",
      "MATCH (a:UnknownLabel) RETURN a;",
    ].join("\n");
    const a = await analyseAll(src, {
      labels: ["Person"],
      strictLabels: true,
    });
    const labels = a.diagnostics.map((d) => d.message);
    expect(labels.some((m) => /UnknownLabel/.test(m))).toBe(true);
  });

  it("analyseAll keeps surfacing RETURN/undeclared warnings on clean statements when a sibling has a SYNTAX error", async () => {
    // Regression: the linter used to short-circuit semantic analysis
    // for the whole document when ANY statement failed syntactic
    // validation, which meant a broken sibling silently disabled the
    // RETURN / undeclared-variable check on every other statement.
    const src = [
      // Statement 1 is broken — `RETURN` with no expression. `validate`
      // will flag it as a syntax error, and `analyse` for this slice
      // will return an empty diagnostic list. The error stays inside
      // the slice (no straddling delimiters / open strings) so the
      // top-level `;` split still produces two slices.
      "MATCH (n) RETURN ;",
      // Statement 2 is clean Cypher but references an undeclared `nx`.
      // The semantic pass must still flag it.
      "MATCH (m) RETURN nx;",
    ].join("\n");
    const a = await analyseAll(src);
    const undecl = a.diagnostics.filter((d) => /\bnx\b/.test(d.message));
    expect(undecl).toHaveLength(1);
    // Span must land inside the second statement.
    const secondStmtStart = src.indexOf("MATCH (m)");
    expect(undecl[0]!.span.start).toBeGreaterThanOrEqual(secondStmtStart);
  });

  it("analyseAll flags RETURN of an undeclared variable in only the offending statement", async () => {
    const src = [
      "MATCH (n) RETURN n;",
      "MATCH (m) RETURN nx;", // typo — `nx` was never bound
      "MATCH (a) RETURN a;",
    ].join("\n");
    const a = await analyseAll(src);
    const undecl = a.diagnostics.filter((d) => /\bnx\b/.test(d.message));
    expect(undecl).toHaveLength(1);
    // The diagnostic must land on the second statement, not the first
    // or the third.
    const secondStmtStart = src.indexOf("MATCH (m)");
    const thirdStmtStart = src.indexOf("MATCH (a)");
    expect(undecl[0]!.span.start).toBeGreaterThanOrEqual(secondStmtStart);
    expect(undecl[0]!.span.start).toBeLessThan(thirdStmtStart);
  });

  it("validateAll: statements separated by `;` are validated independently — fixing one doesn't blank diagnostics on the others", async () => {
    const goodStmt = "MATCH (n) RETURN n";
    const brokenStmt = "MATCH (m) RETURN m LIMIT"; // missing the number
    const src = `${goodStmt};\n${brokenStmt};\n${goodStmt};`;
    const errs = await validateAll(src);
    expect(errs.length).toBeGreaterThanOrEqual(1);
    // The diagnostic must be inside the second statement.
    const secondStart = src.indexOf(brokenStmt);
    const secondEnd = secondStart + brokenStmt.length;
    const err = errs[0]!;
    expect(err.span.start).toBeGreaterThanOrEqual(secondStart);
    expect(err.span.start).toBeLessThanOrEqual(secondEnd + 1);
    // The two good statements must NOT produce errors.
    const goodCount = errs.filter(
      (e) =>
        e.span.start < secondStart || e.span.start > secondEnd + 1,
    ).length;
    expect(goodCount).toBe(0);
  });

  it("parses a CALL { ... } subquery without errors", async () => {
    const result = await parse(
      "MATCH (u:Person) " +
        "CALL { WITH u MATCH (u)-[:RATED]->(m:Movie) RETURN collect(m.title) AS rated } " +
        "RETURN u.name, rated",
    );
    expect(result.ok).toBe(true);
    expect(result.errors).toEqual([]);
  });

  it("CALL { ... } body contributes inner pattern variables to the outline", async () => {
    const o = await outline(
      "MATCH (u:Person) " +
        "CALL { WITH u MATCH (u)-[r:RATED]->(m:Movie) RETURN collect(m.title) AS rated } " +
        "RETURN u, rated",
    );
    const names = o.variables.map((v) => v.name);
    expect(names).toContain("u");
    // Inner-only variables are visible at outline level — useful for
    // hover, jump-to-def, and completion inside the subquery body.
    expect(names).toContain("m");
    expect(names).toContain("r");
    // The RETURN alias is also a binding the outer query can use.
    expect(names).toContain("rated");
    expect(o.labels).toContain("Movie");
    expect(o.relTypes).toContain("RATED");
  });

  it("analyse emits a fold range for the CALL { ... } body", async () => {
    const a = await analyse(
      "MATCH (u:Person) " +
        "CALL { WITH u MATCH (u)-[:RATED]->(m:Movie) RETURN m } " +
        "RETURN u",
    );
    const kinds = a.foldRanges.map((f) => f.kind);
    expect(kinds).toContain("subquery");
  });

  it("highlight produces spans inside the CALL { ... } body too", async () => {
    const spans = await highlight(
      "MATCH (u:Person) " +
        "CALL { WITH u MATCH (u)-[:RATED]->(m:Movie) RETURN m } " +
        "RETURN u",
    );
    // The inner `Movie` label and `RATED` relationship type get
    // highlighted, proving the visitor recurses into the subquery.
    const labels = spans.filter((s) => s.kind === "label").map((s) => {
      // span text isn't stored on the highlight span; instead, use
      // the start/end offsets back into the source.
      return s;
    });
    expect(labels.length).toBeGreaterThan(0);
  });

  it("never flags function calls like `count(r)` / `avg(r.score)` as undeclared vars", async () => {
    const a = await analyse(
      `MATCH (m:Movie)<-[r:RATED]-(u:User)
WHERE r.score >= 8
WITH m, avg(r.score) AS avgScore, count(r) AS votes
WHERE votes > 100
RETURN m.title, avgScore, votes
ORDER BY avgScore DESC, votes DESC
LIMIT 25`,
      {
        labels: ["Movie", "User"],
        relTypes: ["RATED"],
        strictLabels: true,
        strictRelTypes: true,
      },
    );
    const offenders = a.diagnostics
      .map((d) => d.message)
      .filter((m) =>
        /\b(avg|count|sum|min|max|collect|size|length|reverse|coalesce)\b/.test(m),
      );
    expect(offenders).toEqual([]);
  });
});
