import { describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import type { CompletionContext } from "@codemirror/autocomplete";
import { cypherCompletions } from "../src/cypher/completion";
import {
  loraQueryProviders,
  type LoraQueryProviders,
} from "../src/cypher/providers";
import {
  _setOutlineEffect,
  fallbackOutline,
  outlineField,
} from "../src/cypher/scope";

/** Schema used by every test. Mirrors the Storybook story. */
const TEST_PROVIDERS: LoraQueryProviders = {
  labels: ["Person", "Company", "Movie"],
  relTypes: ["KNOWS", "WORKS_AT", "ACTED_IN", "DIRECTED"],
  procedures: [
    { name: "db.indexes", signature: "db.indexes()", info: "List indexes." },
  ],
  getPropertyKeys: (ctx) => {
    if (ctx.kind === "node" && ctx.label === "Person") {
      return ["name", "age", "email", "archived", "createdAt"];
    }
    if (ctx.kind === "node" && ctx.label === "Company") {
      return ["name", "industry", "founded"];
    }
    return [];
  },
};

/**
 * Build an EditorState with our facets and a seeded outline, then
 * synthesise a CompletionContext so we can invoke `cypherCompletions`
 * the same way CodeMirror would.
 */
async function complete(
  doc: string,
  cursor: number,
  explicit = true,
): Promise<{ labels: string[]; options: any[] }> {
  let state = EditorState.create({
    doc,
    selection: { anchor: cursor },
    extensions: [
      outlineField,
      loraQueryProviders.of(TEST_PROVIDERS),
    ],
  });
  // Seed the outline with the regex fallback so scope-aware
  // completions work for partial sources too.
  state = state.update({
    effects: _setOutlineEffect.of(fallbackOutline(doc)),
  }).state;

  const ctx: CompletionContext = {
    state,
    pos: cursor,
    explicit,
    matchBefore(re: RegExp) {
      const line = state.doc.lineAt(cursor);
      const text = state.sliceDoc(line.from, cursor);
      const anchored = new RegExp(
        re.source + "$",
        re.flags.replace("g", ""),
      );
      const m = anchored.exec(text);
      if (!m) return null;
      return { from: cursor - m[0].length, to: cursor, text: m[0] };
    },
    aborted: false,
  } as unknown as CompletionContext;

  let result = cypherCompletions(ctx) as unknown;
  if (result && typeof (result as Promise<unknown>).then === "function") {
    result = await (result as Promise<unknown>);
  }
  if (!result || typeof result !== "object" || !("options" in result)) {
    return { labels: [], options: [] };
  }
  const r = result as { options: any[] };
  return {
    labels: r.options.map((o: { label: string }) => o.label),
    options: r.options,
  };
}

function expectIncludes(labels: string[], ...want: string[]) {
  for (const w of want) expect(labels).toContain(w);
}
function expectExcludes(labels: string[], ...absent: string[]) {
  for (const a of absent) expect(labels).not.toContain(a);
}

describe("cypherCompletions — top-level menu", () => {
  it("offers every clause, snippet, recipe, advanced expr, temporal/spatial, DDL", async () => {
    const { labels } = await complete("", 0);
    expectIncludes(
      labels,
      // clauses
      "MATCH",
      "OPTIONAL MATCH",
      "CREATE",
      "MERGE",
      "WITH",
      "UNWIND",
      "CALL",
      "RETURN",
      // basic snippets
      "MATCH ()-[]->()",
      "CASE",
      "CALL { … }",
      "EXISTS { … }",
      // recipes
      "Recipe: friends of friends",
      "Recipe: paginated list",
      "Recipe: top by aggregate",
      // advanced expression snippets
      "shortestPath((a)-[*]-(b))",
      "allShortestPaths((a)-[*]-(b))",
      "List comprehension",
      "Map projection",
      "Reduce",
      // temporal / spatial
      "date('YYYY-MM-DD')",
      "datetime(...)",
      "duration(P1D)",
      "point({x, y})",
      "point({lat, lon})",
      // query analysis
      "PROFILE",
      "EXPLAIN",
      // DDL
      "CREATE INDEX",
      "CREATE TEXT INDEX",
      "DROP INDEX",
      "SHOW INDEXES",
      "CREATE CONSTRAINT (unique)",
      "CREATE CONSTRAINT (exists)",
      "DROP CONSTRAINT",
      "SHOW CONSTRAINTS",
    );
  });
});

describe("cypherCompletions — function calls auto-open parens", () => {
  it("`count` and namespace members apply as snippets", async () => {
    const { options } = await complete("MATCH (n) RETURN cou", 20);
    const count = options.find((o) => o.label === "count");
    expect(typeof count?.apply).toBe("function");
  });
});

describe("cypherCompletions — pattern starters per clause", () => {
  it.each([
    ["MATCH ", 6],
    ["OPTIONAL MATCH ", 15],
    ["CREATE ", 7],
    ["MERGE ", 6],
  ])("offers (n), (n:Label), (a)-[r]->(b) after `%s`", async (doc, pos) => {
    const { labels } = await complete(doc, pos);
    expectIncludes(labels, "(n)", "(n:Label)", "(a)-[r]->(b)");
  });
});

describe("cypherCompletions — `:` label / rel-type / union", () => {
  it("labels after `(n:`", async () => {
    const { labels } = await complete("MATCH (n:", 9);
    expectIncludes(labels, "Person", "Company", "Movie");
  });
  it("rel types after `[r:`", async () => {
    const { labels } = await complete("MATCH (n)-[r:", 13);
    expectIncludes(labels, "KNOWS", "WORKS_AT", "ACTED_IN", "DIRECTED");
  });
  it("label union after `:Foo|`", async () => {
    const { labels } = await complete("MATCH (n:Person|", 16);
    expectIncludes(labels, "Person", "Company", "Movie");
  });
});

describe("cypherCompletions — fresh names", () => {
  it("inside `MATCH (` suggests fresh single-letter names", async () => {
    const { labels } = await complete("MATCH (", 7);
    expectIncludes(labels, "n", "m", "a", "b", "c");
  });
});

describe("cypherCompletions — pattern continuation after `)`", () => {
  it("offers relationship snippets + follow clauses", async () => {
    const { labels } = await complete("MATCH (n) ", 10);
    expectIncludes(
      labels,
      "-[:TYPE]->",
      "-[r:TYPE]->",
      "<-[:TYPE]-",
      "-[:TYPE]-",
      "-->",
      "<--",
      "--",
      "WHERE",
      "RETURN",
    );
  });
  it("MERGE additionally lists ON CREATE/MATCH SET", async () => {
    const { labels } = await complete("MERGE (n) ", 10);
    expectIncludes(labels, "ON CREATE SET", "ON MATCH SET");
  });
});

describe("cypherCompletions — range literals", () => {
  it("inside `[r ` offers *1..3, *, *0..", async () => {
    const { labels } = await complete("MATCH (n)-[r ", 13);
    expectIncludes(labels, "*1..3", "*", "*0..");
  });
});

describe("cypherCompletions — WHERE clause", () => {
  it("WHERE | suggests in-scope variable + EXISTS", async () => {
    const { labels } = await complete("MATCH (n:Person) WHERE ", 23);
    expectIncludes(labels, "n", "EXISTS { … }", "NOT EXISTS { … }");
  });
  it("after a value, boolean + comparison operators", async () => {
    const { labels } = await complete("MATCH (n) WHERE n.age > 18 ", 27);
    expectIncludes(
      labels,
      "AND",
      "OR",
      "=",
      "<>",
      "<",
      ">",
      "IN",
      "STARTS WITH",
      "ENDS WITH",
      "CONTAINS",
    );
  });
  it("after `IS ` offers NULL / NOT NULL", async () => {
    const { labels } = await complete("MATCH (n) WHERE n IS ", 21);
    expectIncludes(labels, "NULL", "NOT NULL");
  });
  it("after `= ` offers literal RHS values", async () => {
    const { labels } = await complete("MATCH (n) WHERE n.age = ", 24);
    expectIncludes(labels, "TRUE", "FALSE", "NULL");
  });

  it("after `<prop> = ` leads with the matching `$<prop>` parameter", async () => {
    // `n.email = |` — the LHS property is `email`, so the most
    // useful RHS is `$email`. It should sort to the very top.
    const { options } = await complete("MATCH (n) WHERE n.email = ", 26);
    expect(options.length).toBeGreaterThan(0);
    const top = options[0]!;
    expect(top.label).toBe("$email");
  });

  it("after `<var>.<prop> ` offers operator-with-rhs snippets", async () => {
    // `n.name ` triggers afterValue in WHERE → operator+rhs snippets
    // surface alongside the bare operators.
    const { labels } = await complete("MATCH (n) WHERE n.name ", 23);
    expectIncludes(
      labels,
      "= $name",
      "IS NULL",
      "IS NOT NULL",
      "STARTS WITH '…'",
      "CONTAINS '…'",
    );
  });
});

describe("cypherCompletions — RETURN AS-alias", () => {
  it("after `RETURN n.name ` suggests `AS name`", async () => {
    const { labels } = await complete("MATCH (n) RETURN n.name ", 24);
    expectIncludes(labels, "AS name", "AS <alias>");
  });

  it("after `WITH n.email ` suggests `AS email`", async () => {
    const { labels } = await complete("MATCH (n) WITH n.email ", 23);
    expectIncludes(labels, "AS email");
  });
});

describe("cypherCompletions — RETURN / WITH / YIELD", () => {
  it("RETURN | leads with `*` and auto-fill", async () => {
    const { labels } = await complete("MATCH (n) RETURN ", 17);
    expectIncludes(labels, "*", "count(*) AS total");
  });
  it("WITH | leads with `*`", async () => {
    const { labels } = await complete("MATCH (n) WITH ", 15);
    expectIncludes(labels, "*");
  });
  it("YIELD | leads with `*`", async () => {
    const { labels } = await complete("CALL db.proc() YIELD ", 21);
    expectIncludes(labels, "*");
  });
  it("after RETURN <expr> offers UNION / UNION ALL", async () => {
    const { labels } = await complete("MATCH (n) RETURN n ", 19);
    expectIncludes(labels, "UNION", "UNION ALL");
  });
});

describe("cypherCompletions — ORDER BY position aware", () => {
  it("at expression start: variables + ASC/DESC", async () => {
    const { labels } = await complete(
      "MATCH (n:Person) RETURN n ORDER BY ",
      35,
    );
    expectIncludes(labels, "n DESC", "n ASC");
  });
  it("after a value: only ASC/DESC", async () => {
    const { labels } = await complete(
      "MATCH (n) RETURN n ORDER BY n.age ",
      34,
    );
    expectIncludes(labels, "DESC", "ASC");
  });
});

describe("cypherCompletions — YIELD columns from procedure signatures", () => {
  it("CALL db.indexes() YIELD | offers the procedure's columns + *", async () => {
    const { labels } = await complete(
      "CALL db.indexes() YIELD ",
      24,
    );
    // db.indexes has no parsed signature in TEST_PROVIDERS so columns
    // are empty — we should still get `*` so the rest of the flow
    // doesn't lock up.
    expectIncludes(labels, "*");
  });

  it("CALL db.indexes() YIELD | with a parseable `:: (cols)` signature surfaces each column", async () => {
    // Re-mount the editor with a richer signature.
    let state = EditorState.create({
      doc: "CALL db.indexes() YIELD ",
      selection: { anchor: 24 },
      extensions: [
        outlineField,
        loraQueryProviders.of({
          labels: [],
          relTypes: [],
          procedures: [
            {
              name: "db.indexes",
              signature: "db.indexes() :: (name, state, type)",
            },
          ],
        }),
      ],
    });
    state = state.update({
      effects: _setOutlineEffect.of(fallbackOutline("CALL db.indexes() YIELD ")),
    }).state;
    const ctx: CompletionContext = {
      state,
      pos: 24,
      explicit: true,
      matchBefore(re: RegExp) {
        const line = state.doc.lineAt(24);
        const text = state.sliceDoc(line.from, 24);
        const anchored = new RegExp(re.source + "$", re.flags.replace("g", ""));
        const m = anchored.exec(text);
        if (!m) return null;
        return { from: 24 - m[0].length, to: 24, text: m[0] };
      },
      aborted: false,
    } as unknown as CompletionContext;
    let r = cypherCompletions(ctx) as unknown;
    if (r && typeof (r as Promise<unknown>).then === "function") {
      r = await (r as Promise<unknown>);
    }
    const labels = ((r as { options: { label: string }[] }).options ?? []).map(
      (o) => o.label,
    );
    expectIncludes(labels, "name", "state", "type", "*");
  });
});

describe("cypherCompletions — aggregates", () => {
  it("count( → DISTINCT + *", async () => {
    const { labels } = await complete("MATCH (n) RETURN count(", 23);
    expectIncludes(labels, "DISTINCT", "*");
  });
  it.each(["sum", "min", "max", "collect", "avg"])(
    "%s( → DISTINCT but not *",
    async (fn) => {
      const doc = `MATCH (n) RETURN ${fn}(`;
      const { labels } = await complete(doc, doc.length);
      expectIncludes(labels, "DISTINCT");
      expectExcludes(labels, "*");
    },
  );
  it.each(["percentileCont", "percentileDisc"])(
    "%s( → neither DISTINCT nor *",
    async (fn) => {
      const doc = `MATCH (n) RETURN ${fn}(`;
      const { labels } = await complete(doc, doc.length);
      expectExcludes(labels, "DISTINCT", "*");
    },
  );
});

describe("cypherCompletions — CASE flow", () => {
  it("inside CASE: WHEN / ELSE / END / THEN", async () => {
    const { labels } = await complete(
      "MATCH (n) RETURN CASE n.age WHEN 1 ",
      35,
    );
    expectIncludes(labels, "WHEN", "ELSE", "END", "THEN");
  });
});

describe("cypherCompletions — namespace members", () => {
  it("math.", async () => {
    const { labels } = await complete("MATCH (n) RETURN math.", 22);
    expectIncludes(labels, "abs", "sqrt", "floor", "ceil", "min", "max");
  });
  it("string.", async () => {
    const { labels } = await complete("MATCH (n) RETURN string.", 24);
    // Names below match the canonical `string.*` registry in
    // `lora-builtins-meta`. Cypher-style camelCase aliases like
    // `startsWith` / `endsWith` would need an entry in BUILTIN_ALIASES
    // — they are not direct namespace members.
    expectIncludes(
      labels,
      "upper",
      "lower",
      "length",
      "contains",
      "starts_with",
      "ends_with",
      "split",
      "trim",
      "replace",
    );
  });
  it("list.", async () => {
    const { labels } = await complete("MATCH (n) RETURN list.", 22);
    expectIncludes(
      labels,
      "sum",
      "avg",
      "size",
      "append",
      "first",
      "last",
      "contains",
      "reverse",
    );
  });
  it("map.", async () => {
    const { labels } = await complete("MATCH (n) RETURN map.", 21);
    expectIncludes(labels, "keys", "size");
  });
  it("temporal.", async () => {
    const { labels } = await complete("MATCH (n) RETURN temporal.", 26);
    // `date` / `datetime` are top-level *aliases* (added in 0.11.x) —
    // they live in CYPHER_ALIASES, not as members of `temporal.*`.
    // The namespace itself surfaces only the registered builtins.
    expectIncludes(labels, "now", "today", "parse", "format");
  });
});

describe("cypherCompletions — registry coverage", () => {
  it("surfaces namespaces that previously had zero hand-written members", async () => {
    // Pre-0.12 `data.ts` listed these namespaces with no members, so
    // `vector.|` / `crypto.|` returned nothing. Once the registry is
    // sourced from `lora-builtins-meta`, the real members appear.
    const v = await complete("RETURN vector.", 14);
    expectIncludes(v.labels, "distance", "similarity", "dimension");
    const c = await complete("RETURN crypto.", 14);
    expectIncludes(c.labels, "blake3", "crc32");
  });
  it("date alias is offered at the top level", async () => {
    // `date(...)` is a Cypher-style alias to `temporal.now`. It must
    // be reachable from the top-level expression completion list.
    const { labels } = await complete("RETURN da", 9);
    expectIncludes(labels, "date");
  });
});

describe("cypherCompletions — var.property", () => {
  it("n.| with Person label returns the schema keys", async () => {
    const { labels } = await complete("MATCH (n:Person) RETURN n.name", 26);
    expectIncludes(
      labels,
      "name",
      "age",
      "email",
      "archived",
      "createdAt",
    );
  });
  it("alias.| follows WITH AS source label", async () => {
    const { labels } = await complete(
      "MATCH (alice:Person) WITH alice AS friend RETURN friend.name",
      56,
    );
    expectIncludes(labels, "name", "age", "email");
  });
  it("r.| (rel with no schema) falls back to bare property names — never `key: value`", async () => {
    const { labels } = await complete(
      "MATCH (n)-[r:KNOWS]->(m) WHERE r.name = 1",
      33,
    );
    expectIncludes(labels, "name", "id");
    // The map-entry placeholder must not appear in expression position.
    expectExcludes(labels, "<key>: <value>");
  });
  it("nested n.foo.| → no completion (no nested schema)", async () => {
    const { labels } = await complete(
      "MATCH (n:Person) RETURN n.foo.name",
      30,
    );
    expectExcludes(labels, "name", "age", "email");
  });

  it("WHERE derp.| via WITH AS alias on a label-less node → bare property names, never `key: value`", async () => {
    const doc =
      "MATCH (n:Person {name: 'Alice'})-[r:KNOWS]->(m)\nWITH m AS derp\nWHERE derp.\nRETURN n";
    const cursor = doc.indexOf("derp.\n") + 5;
    const { labels, options } = await complete(doc, cursor);
    // Bare property names show up
    expectIncludes(labels, "name", "id");
    // The map-entry form must NEVER surface in expression position
    expectExcludes(labels, "<key>: <value>");
    // And none of the offered completions should insert `: value`
    for (const o of options) {
      const apply = o.apply;
      const inserts =
        typeof apply === "string"
          ? apply
          : typeof apply === "function"
            ? "(snippet)"
            : o.label;
      expect(inserts).not.toMatch(/:\s*['"]?value/);
      expect(inserts).not.toMatch(/:\s*'value'/);
    }
  });
});

describe("cypherCompletions — property maps", () => {
  it("inside `(n:Person {` returns Person's keys", async () => {
    const { labels } = await complete("MATCH (n:Person {", 17);
    expectIncludes(labels, "name", "age", "email");
  });

  it("apply text is `key: ` when the key is being typed fresh", async () => {
    const { options } = await complete("MATCH (n:Person {nam", 20);
    const nameOpt = options.find((o) => o.label === "name");
    expect(nameOpt).toBeDefined();
    expect(nameOpt!.apply).toBe("name: ");
  });

  it("drops the `: ` scaffold when a `:` already follows the cursor", async () => {
    // Cursor sits inside `nam|: 'Alice'` — picking `name` would
    // otherwise emit `name: : 'Alice'`.
    const src = "MATCH (n:Person {nam: 'Alice'})";
    const cursor = src.indexOf("nam:") + 3; // right after `nam`, before `:`
    const { options, labels } = await complete(src, cursor);
    const nameOpt = options.find((o) => o.label === "name");
    expect(nameOpt).toBeDefined();
    expect(nameOpt!.apply).toBe("name");
    // The generic `<key>: <value>` scaffold doesn't make sense here
    // — there's already a value present — and would otherwise produce
    // a stray double-colon.
    expectExcludes(labels, "<key>: <value>");
  });

  it("keeps the `key: ` apply for the SECOND key after a comma", async () => {
    // `{name: 'Alice', |}` — the new key has no colon ahead yet.
    const src = "MATCH (n:Person {name: 'Alice', })";
    const cursor = src.indexOf(", ") + 2; // cursor right after the comma + space
    const { options } = await complete(src, cursor);
    const emailOpt = options.find((o) => o.label === "email");
    expect(emailOpt).toBeDefined();
    expect(emailOpt!.apply).toBe("email: ");
  });
});

describe("cypherCompletions — SET item snippets", () => {
  it("SET | with scope vars surfaces concrete name snippets", async () => {
    const { labels } = await complete(
      "MATCH (alice:Person) SET ",
      25,
    );
    expectIncludes(labels, "alice.<key> = <value>", "alice += { … }");
  });
});

describe("cypherCompletions — safety (never recommend the impossible)", () => {
  it("inside a string literal returns nothing", async () => {
    const { labels } = await complete(
      "MATCH (n) WHERE n.name = 'hello",
      31,
    );
    expectExcludes(labels, "MATCH", "WHERE", "*");
  });
  it("inside a // comment returns nothing", async () => {
    const { labels } = await complete("MATCH (n) // n.", 15);
    expectExcludes(labels, "MATCH", "name", "*");
  });
  it("inside a /* block */ comment returns nothing", async () => {
    const { labels } = await complete("MATCH (n) /* n.", 15);
    expectExcludes(labels, "MATCH", "name", "*");
  });
});

describe("cypherCompletions — mid-typing filtering surface", () => {
  it("typing `co` after RETURN surfaces count + collect + CASE", async () => {
    const { labels } = await complete("MATCH (n) RETURN co", 19);
    expectIncludes(labels, "count", "collect", "CASE");
  });
});

describe("cypherCompletions — LIMIT / SKIP numeric helpers", () => {
  it.each(["LIMIT ", "SKIP "])(
    "after `%s`, suggest pagination sizes",
    async (kw) => {
      const doc = `MATCH (n) RETURN n ${kw}`;
      const { labels } = await complete(doc, doc.length);
      expectIncludes(labels, "10", "25", "50", "100", "500", "1000");
    },
  );
});

describe("cypherCompletions — pattern flow continuations", () => {
  it.each([
    ["MATCH (n)-[r]->", 15],
    ["MATCH (n)-->", 12],
    ["MATCH (n)<--", 12],
    ["MATCH (n)--", 11],
  ])("after `%s` suggests node-pattern starters", async (doc, pos) => {
    const { labels } = await complete(doc, pos);
    expectIncludes(labels, "(n)", "(n:Label)");
  });

  it("multi-pipe label union — `:Person|Company|` offers more labels", async () => {
    const { labels } = await complete("MATCH (n:Person|Company|", 24);
    expectIncludes(labels, "Person", "Company", "Movie");
  });
});

describe("cypherCompletions — new snippet recipes", () => {
  it("top-level lists `MATCH path = (a)-[*]->(b)`", async () => {
    const { labels } = await complete("", 0);
    expectIncludes(labels, "MATCH path = (a)-[*]->(b)");
  });
  it.each([
    "WHERE n.x IS NULL",
    "WHERE n.x IS NOT NULL",
    "WHERE n.x IN […]",
    "WHERE NOT EXISTS pattern",
  ])("top-level lists `%s`", async (label) => {
    const { labels } = await complete("", 0);
    expectIncludes(labels, label);
  });
});
