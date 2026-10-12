// G-48: a trigger knows which relationship a mutation wrote. A trigger
// function reads them in `change`; a `@trigger(on: [CONNECT], field:)`
// statement runs once per relationship, with `this` and `target` bound.

import { describe, expect, test } from "vitest";
import { buildModel, ModelError, type WriteChange } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const typeDefs = /* GraphQL */ `
  type Claims @jwt {
    sub: String! @viewer(type: "P", field: "key")
  }
  interface Thing {
    key: String!
  }
  type Book implements Thing @node @mutation {
    key: String! @key
  }
  type Film implements Thing @node @mutation {
    key: String! @key
  }
  type P
    @node
    @mutation
    @trigger(
      on: [CONNECT]
      field: "follows"
      statement: """
      CREATE (n:Note {
        key: 'followed:' + this.key + ':' + target.key,
        to: target.key, by: $viewer, operation: $operation
      })
      RETURN n AS created
      """
    )
    @trigger(
      on: [DISCONNECT]
      field: "follows"
      statement: """
      MATCH (n:Note {key: 'followed:' + this.key + ':' + target.key})
      SET n.operation = $operation
      RETURN n AS updated
      """
    )
    @trigger(
      on: [CONNECT]
      field: "likes"
      statement: """
      CREATE (n:Note {key: 'liked:' + this.key + ':' + target.key, to: labels(target)[0]})
      RETURN n AS created
      """
    ) {
    key: String! @key
    follows: [P!]!
      @relationship(type: "FOLLOWS", direction: OUT, properties: "Since")
    followers: [P!]!
      @relationship(type: "FOLLOWS", direction: IN, properties: "Since")
    likes: [Thing!]! @relationship(type: "LIKES", direction: OUT)
    blocks: [P!]! @relationship(type: "BLOCKS", direction: OUT)
  }
  type Since @relationshipProperties {
    year: Int
  }
  type Note @node @subscription(operations: [CREATE]) {
    key: String! @key
    to: String
    by: String
    operation: String
  }
`;

async function setup(
  seen?: Array<{ connected: number; disconnected: number; types: string[] }>,
) {
  const t = await createTestLoraGraphQL({
    typeDefs,
    seed: [
      "UNWIND ['a','b','c','d'] AS k CREATE (:P {key: k})",
      "CREATE (:Book {key: 'b1'}), (:Film {key: 'f1'})",
    ],
    ...(seen
      ? {
          triggers: {
            P: {
              update: ({ change }: { change: WriteChange }) => {
                seen.push({
                  connected: change.connected.length,
                  disconnected: change.disconnected.length,
                  types: change.relationshipTypes,
                });
              },
            },
          },
        }
      : {}),
  });
  const changes: WriteChange[] = [];
  t.lora.onWrite((c) => changes.push(c));
  const run = async (source: string, sub = "a") => {
    const r = await t.lora.execute({ source, context: { jwt: { sub } } });
    expect(r.errors).toBeUndefined();
    return r.data as Record<string, unknown>;
  };
  const notes = async () =>
    (
      (await run(`{ notes(sort: [{ key: ASC }]) { key to by operation } }`))[
        "notes"
      ] as Array<Record<string, unknown>>
    ).map((n) => n);
  return { t, run, notes, changes };
}

describe("a trigger function", () => {
  test("reads the relationships its mutation connected and disconnected", async () => {
    const seen: Array<{
      connected: number;
      disconnected: number;
      types: string[];
    }> = [];
    const { run, changes } = await setup(seen);
    await run(
      `mutation { updateP(key: "a", update: { blocks: { connect: [{ key: "b" }] } }) { p { key } } }`,
    );
    await run(
      `mutation { updateP(key: "a", update: { blocks: { disconnect: ["b"] } }) { p { key } } }`,
    );
    expect(seen).toEqual([
      { connected: 1, disconnected: 0, types: ["BLOCKS"] },
      { connected: 0, disconnected: 1, types: ["BLOCKS"] },
    ]);
    // The committed change lists each once.
    expect(
      changes.map((c) => [c.connected.length, c.disconnected.length]),
    ).toEqual([
      [1, 0],
      [0, 1],
    ]);
  });
});

describe("@trigger(on: [CONNECT], field:)", () => {
  test("one node per relationship connected, reading no other", async () => {
    const { run, notes, changes } = await setup();
    await run(
      `mutation { updateP(key: "a", update: { follows: { connect: [{ key: "b" }, { key: "c" }] } }) { p { key } } }`,
    );
    expect(await notes()).toEqual([
      { key: "followed:a:b", to: "b", by: "a", operation: "CONNECT" },
      { key: "followed:a:c", to: "c", by: "a", operation: "CONNECT" },
    ]);
    expect(changes[0]!.broad).toBe(false);
    expect(changes[0]!.created).toEqual([
      { type: "Note", key: "followed:a:b" },
      { type: "Note", key: "followed:a:c" },
    ]);
    // A later follow fires for its own relationship only.
    await run(
      `mutation { updateP(key: "a", update: { follows: { connect: [{ key: "d" }] } }) { p { key } } }`,
    );
    expect((await notes()).map((n) => n["key"])).toEqual([
      "followed:a:b",
      "followed:a:c",
      "followed:a:d",
    ]);
  });

  test("connecting a connected pair again fires nothing", async () => {
    const { run, notes } = await setup();
    const follow = `mutation { updateP(key: "a", update: { follows: { connect: [{ key: "b", edge: { year: 2026 } }] } }) { p { key } } }`;
    await run(follow);
    await run(follow);
    expect((await notes()).map((n) => n["key"])).toEqual(["followed:a:b"]);
  });

  test("a relationship written from the other side's field fires it too", async () => {
    const { run, notes } = await setup();
    await run(
      `mutation { updateP(key: "b", update: { followers: { connect: [{ key: "a" }] } }) { p { key } } }`,
      "b",
    );
    // `this` is the follower, whichever field wrote the relationship.
    expect(await notes()).toEqual([
      { key: "followed:a:b", to: "b", by: "b", operation: "CONNECT" },
    ]);
  });

  test("a create that connects, and a nested create, fire it", async () => {
    const { run, notes } = await setup();
    await run(
      `mutation { createPs(input: [{ key: "n", follows: { connect: [{ key: "a" }], create: [{ node: { key: "m" } }] } }]) { ps { key } } }`,
    );
    expect((await notes()).map((n) => n["key"])).toEqual([
      "followed:n:a",
      "followed:n:m",
    ]);
  });

  test("an interface field fires per member, with target the member's node", async () => {
    const { run, notes } = await setup();
    await run(
      `mutation { updateP(key: "a", update: { likes: { connect: { Book: [{ key: "b1" }], Film: [{ key: "f1" }] } } }) { p { key } } }`,
    );
    expect((await notes()).map((n) => [n["key"], n["to"]])).toEqual([
      ["liked:a:b1", "Book"],
      ["liked:a:f1", "Film"],
    ]);
  });

  test("another relationship of the type fires nothing", async () => {
    const { run, notes } = await setup();
    await run(
      `mutation { updateP(key: "a", update: { blocks: { connect: [{ key: "b" }] } }) { p { key } } }`,
    );
    expect(await notes()).toEqual([]);
  });

  test("a subscription hears what it created", async () => {
    const { t, run } = await setup();
    const events = (await t.lora.subscribe({
      source: `subscription { noteChanged { operation key } }`,
    })) as AsyncIterableIterator<{ data?: unknown }>;
    const next = events.next();
    await run(
      `mutation { updateP(key: "a", update: { follows: { connect: [{ key: "b" }] } }) { p { key } } }`,
    );
    expect((await next).value.data).toEqual({
      noteChanged: { operation: "CREATE", key: "followed:a:b" },
    });
    await events.return?.();
  });
});

describe("@trigger(on: [DISCONNECT], field:)", () => {
  test("fires per relationship removed, both ends still there", async () => {
    const { run, notes } = await setup();
    await run(
      `mutation { updateP(key: "a", update: { follows: { connect: [{ key: "b" }, { key: "c" }] } }) { p { key } } }`,
    );
    await run(
      `mutation { updateP(key: "a", update: { follows: { disconnect: ["b"] } }) { p { key } } }`,
    );
    expect((await notes()).map((n) => [n["key"], n["operation"]])).toEqual([
      ["followed:a:b", "DISCONNECT"],
      ["followed:a:c", "CONNECT"],
    ]);
  });

  test("a relationship that goes with a deleted node fires nothing", async () => {
    const { run, notes } = await setup();
    await run(
      `mutation { updateP(key: "a", update: { follows: { connect: [{ key: "b" }] } }) { p { key } } }`,
    );
    await run(`mutation { deleteP(key: "b") { nodesDeleted } }`);
    expect((await notes()).map((n) => [n["key"], n["operation"]])).toEqual([
      ["followed:a:b", "CONNECT"],
    ]);
  });
});

describe("at startup", () => {
  const problems = (trigger: string): string[] => {
    try {
      buildModel(`type P @node @mutation ${trigger} {
        key: String! @key
        name: String
        follows: [P!]! @relationship(type: "F", direction: OUT)
      }`);
      return [];
    } catch (err) {
      return (err as ModelError).problems.map((p) => p.message);
    }
  };
  const statement = `statement: "RETURN this AS updated"`;

  test("a relationship trigger names its field, and nothing else", () => {
    expect(
      problems(
        `@trigger(on: [CONNECT, DISCONNECT], field: "follows", ${statement})`,
      ),
    ).toEqual([]);
    expect(problems(`@trigger(on: [CONNECT], ${statement})`)).toEqual([
      "@trigger: CONNECT needs field: the relationship field it fires for",
    ]);
    expect(
      problems(`@trigger(on: [CONNECT], field: "name", ${statement})`),
    ).toEqual(["@trigger: name is not a @relationship field"]);
    expect(
      problems(`@trigger(on: [CONNECT], field: "nope", ${statement})`),
    ).toEqual(["@trigger: P has no field nope"]);
    expect(
      problems(
        `@trigger(on: [CREATE, CONNECT], field: "follows", ${statement})`,
      ),
    ).toEqual([
      "@trigger: CONNECT and DISCONNECT run per relationship, the others per node: declare two triggers",
    ]);
    expect(
      problems(`@trigger(on: [UPDATE], field: "follows", ${statement})`),
    ).toEqual([
      "@trigger: field names the relationship of a CONNECT or DISCONNECT trigger; fields narrows UPDATE",
    ]);
  });

  test("check() plans the statement with this and target bound", async () => {
    const lora = (
      await createTestLoraGraphQL({
        typeDefs: `type P @node @mutation
          @trigger(on: [CONNECT], field: "follows", statement: "SET target.seen = this.key RETURN target AS updated")
          @trigger(on: [CONNECT], field: "follows", statement: "SET nobody.seen = 1 RETURN this AS updated") {
          key: String! @key
          follows: [P!]! @relationship(type: "F", direction: OUT)
        }`,
      })
    ).lora;
    const findings = (await lora.check()).cypher;
    expect(findings.map((f) => f.field)).toEqual(["@trigger 2"]);
  });
});
