---
title: Testing a LoraGraphQL API
sidebar_label: Testing
description: Test a @loradb/lora-graphql API against an in-memory LoraDB with createTestLoraGraphQL, assert the Cypher it runs, and fail tests when a query stops using its index with expectSeeks.
---

# Testing

`@loradb/lora-graphql/testing` has two helpers:

- `createTestLoraGraphQL` builds the API over a fresh in-memory LoraDB,
  with every index and constraint created and your seed data loaded.
- `expectSeeks` fails when a query's statements stop using the index
  access they were compiled for.

Both need `@loradb/lora-node` (a dev dependency is enough) and work with
any test runner: failures are plain thrown errors.

```bash
npm install --save-dev @loradb/lora-node
```

## A test database

```ts title="festivals.test.ts"
import { afterAll, beforeAll, expect, test } from "vitest";
import {
  createTestLoraGraphQL,
  type TestLoraGraphQL,
} from "@loradb/lora-graphql/testing";

const typeDefs = /* GraphQL */ `
  type Festival @node @mutation {
    key: String! @key
    name: String! @filterable(byValue: [EQ, CONTAINS]) @sortable
    capacity: Int @filterable(byValue: [GTE])
  }
`;

let t: TestLoraGraphQL;

beforeAll(async () => {
  t = await createTestLoraGraphQL({
    typeDefs,
    seed: [
      "CREATE (:Festival {key: 'f1', name: 'Sunland', capacity: 5000})",
      "CREATE (:Festival {key: 'f2', name: 'Moonfest', capacity: 800})",
    ],
  });
});

afterAll(() => t.close());

test("filters by capacity", async () => {
  const data = await t.data(
    `query ($min: Int) { festivals(where: { capacity: { gte: $min } }) { key } }`,
    { min: 1000 },
  );
  expect(data).toEqual({ festivals: [{ key: "f1" }] });
});
```

`createTestLoraGraphQL(options)` takes every
[`LoraGraphQL` option](/docs/graphql/api-reference#new-loragraphqloptions)
except `driver`, plus `seed`:

| `seed` | Runs |
| --- | --- |
| a string | One Cypher statement |
| an array of strings | Each statement, in order |
| `async (db) => { ... }` | Your function, with the database, for parameters or anything else |

The seed runs after the schema is asserted, so it runs against the same
constraints as production: a seed that violates a `@key` fails.

It returns:

| Field | Is |
| --- | --- |
| `lora` | The `LoraGraphQL` instance |
| `db` | The in-memory database, for raw Cypher (`await t.db.execute(...)`) |
| `schema` | `lora.getSchema()` |
| `run(source, variables?, context?)` | Executes a document and returns the full result; errors stay in `result.errors` |
| `data(source, variables?, context?)` | Executes a document and returns `data`; throws the first error |
| `statements` | Every statement run so far, as `{ field, statement: { text, params } }` |
| `close()` | Releases the database |

Each call to `createTestLoraGraphQL` gets its own database. Create one per
test file (or per test, when tests write), and close it afterwards.

## Testing errors and rules

Pass claims as the context to test authorization, and read error codes
from `run()`. This test assumes `Festival` also has
`@authentication(operations: [CREATE], jwt: { roles: { includes: "editor" } })`:

```ts
test("editors only", async () => {
  const create = `mutation { createFestivals(input: [{ key: "f3", name: "X" }]) { info { nodesCreated } } }`;

  const anonymous = await t.run(create);
  expect(anonymous.errors?.[0]?.extensions?.code).toBe("UNAUTHENTICATED");

  const editor = await t.data(create, {}, { jwt: { sub: "u1", roles: ["editor"] } });
  expect(editor).toEqual({ createFestivals: { info: { nodesCreated: 1 } } });
});

test("pages are bounded", async () => {
  const r = await t.run(`{ festivals(limit: 500) { key } }`);
  expect(r.errors?.[0]?.extensions?.code).toBe("LIMIT_EXCEEDED");
});
```

`run()` and `data()` execute through `graphql-js` directly, not through
`lora.execute()`, so document guards and persisted-only mode do not
apply. Test those with `t.lora.execute({ source, context })`.

## Asserting the Cypher

`t.statements` records every statement, which makes it easy to pin what a
query compiles to:

```ts
test("one statement per root field", async () => {
  t.statements.length = 0; // clear
  await t.data(`{ festivals { key } festival(key: "f1") { name } }`);
  expect(t.statements).toHaveLength(2);
  expect(t.statements[0]!.statement.text).toMatchInlineSnapshot();
});
```

For a query that has not run, `t.lora.compile(document, variables)`
returns the statements without executing them. Snapshot tests over
compiled Cypher catch unintended changes to the translation, at the cost
of updating snapshots when you upgrade the package.

## Asserting index use

```ts
import { expectSeeks } from "@loradb/lora-graphql/testing";

test("name search uses the TEXT index", async () => {
  await expectSeeks(
    t.lora,
    `{ festivals(where: { name: { contains: "land" } }) { key } }`,
  );
});
```

`expectSeeks(lora, document, variables?, options?)` plans every statement
of every root field and throws unless each one uses the access it was
compiled for. When it fails, it names the root field, the expected seek
and the statement:

```text
expected every root field to seek:
festivals: expected an index text seek on :Festival (Festival.name contains), plan uses NodeByLabelScan
  MATCH (this:Festival)
  WHERE this.name CONTAINS $p0
  WITH this ORDER BY this.key ASC LIMIT $p1
  RETURN this { .key } AS this
```

Options:

- `rowBudget`: also fail when the engine estimates more rows than this.
- `context`: plan the statements a given caller gets (for example
  `{ jwt }`), since claim checks change the statement text.

`expectSeeks` takes queries only. It is the per-test version of
[`lora-graphql check`](/docs/graphql/cli#check); use `check` in CI to
cover every operation your clients send, and `expectSeeks` for the few
queries whose plans you want to pin next to the code.

## Tips

- **Seed enough data to matter.** Plans are checked against the engine's
  planner, which uses the indexes regardless of size, but row estimates
  (`rowBudget`) and cost statistics (`t.lora.analyze()`) need realistic
  data.
- **Test with the production options.** Pass the same `maxLimit`,
  `maxCost`, `callbacks`, `resolvers` and `scalars` you use in
  production, so tests hit the same limits and code paths.
- **Change events.** `t.lora.onWrite(fn)` and `t.lora.changes()` work in
  tests, so you can assert the exact write-set of a mutation.
- **Keep versions in lockstep.** Test against the same
  `@loradb/lora-node` version you deploy.
