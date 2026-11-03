// What a read-only `@cypher` statement reads, told from its text (G-78):
// labels and relationship types where the patterns name them, opaque
// (undefined) wherever they do not.

import { describe, expect, test } from "vitest";
import { statementReads } from "../src/model/cypher-reads.js";

const reads = (statement: string, bound: string[] = ["this"]) =>
  statementReads(statement, bound);

describe("statementReads", () => {
  test.each([
    [
      "MATCH (a:ImageAsset {source: this.image}) RETURN a LIMIT 1",
      ["ImageAsset"],
      [],
    ],
    [
      "MATCH (this)<-[:PLAYS_AT]-(a:Artist:Act) RETURN count(a) AS n",
      ["Act", "Artist"],
      ["PLAYS_AT"],
    ],
    [
      "MATCH (this)-[r:A|:B*1..3]->(x:X|Y) RETURN x.key AS k",
      ["X", "Y"],
      ["A", "B"],
    ],
    [
      "RETURN size([(this)-[:TAGGED]->(t:Tag) WHERE t.on | t.key]) AS n",
      ["Tag"],
      ["TAGGED"],
    ],
    ["RETURN COUNT { (this)-[:IN]->(:City) } AS n", ["City"], ["IN"]],
    [
      "MATCH (f:Festival) WHERE EXISTS { (f)-[:AT]->(v:Venue) } RETURN f",
      ["Festival", "Venue"],
      ["AT"],
    ],
    ["MATCH (`odd label`:`Odd Label`) RETURN `odd label`", ["Odd Label"], []],
    // A string or comment that looks like a pattern is not one.
    [
      "MATCH (a:A) WHERE a.s <> '(x)--(y)' /* (z) */ RETURN a // CALL foo()",
      ["A"],
      [],
    ],
    [
      "MATCH (a:A) WITH a, count(a) AS n MATCH (a)-[:R]->(b:B) RETURN n, b",
      ["A", "B"],
      ["R"],
    ],
    [
      "MATCH (a:A) WITH collect(a) AS xs UNWIND xs AS x MATCH (x)-[:R]->(:B) RETURN x",
      ["A", "B"],
      ["R"],
    ],
    [
      "CALL { WITH this MATCH (this)-[:R]->(b:B) RETURN b } RETURN b",
      ["B"],
      ["R"],
    ],
    [
      "MATCH (a:A) RETURN CAST(a.n AS INTEGER) AS n, coalesce(a.x, toString(a.y)) AS s, timestamp() AS t",
      ["A"],
      [],
    ],
  ])("%s", (statement, labels, relationships) => {
    expect(reads(statement)).toEqual({ labels, relationships });
  });

  test.each([
    // A procedure can read anything.
    "CALL db.index.fulltext.queryNodes('i', 'q') YIELD node RETURN node",
    // A node with no label on a variable nothing bound.
    "MATCH (n) RETURN count(n) AS n",
    "MATCH (this)-[:R]->(n) RETURN n.key AS k",
    "MATCH (this)-[:R]->() RETURN count(*) AS n",
    // A relationship of any type.
    "MATCH (this)-->(b:B) RETURN b",
    "MATCH (this)-[r]->(b:B) RETURN b",
    "MATCH (this)-[*1..2]->(b:B) RETURN b",
    // A WITH drops what it does not project: `a` is a new node below.
    "MATCH (a:A) WITH count(a) AS n MATCH (a) RETURN n, a.v",
    // A `CALL { }` body sees only what it imports.
    "MATCH (a:A) CALL { MATCH (a) RETURN a.v AS v } RETURN v",
    "CALL { MATCH (this)-[:R]->(b:B) RETURN b } RETURN b",
    // Each UNION branch imports for itself.
    "MATCH (this)-[:R]->(b:B) RETURN b UNION MATCH (this)-[:S]->(b:B) RETURN b",
  ])("opaque: %s", (statement) => {
    expect(reads(statement)).toBeUndefined();
  });

  test("a root field has no `this`", () => {
    expect(reads("MATCH (this)-[:R]->(b:B) RETURN b", [])).toBeUndefined();
    expect(reads("MATCH (p:Post) RETURN count(p) AS n", [])).toEqual({
      labels: ["Post"],
      relationships: [],
    });
  });
});
