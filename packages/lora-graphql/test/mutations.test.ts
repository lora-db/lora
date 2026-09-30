import type { WriteChange } from "../src/index.js";
import { appTypeDefs } from "./fixtures.js";
import { festivalHarness, type Harness } from "./harness.js";
import { unknownField } from "./graphql-messages.js";

let h: Harness;
const changes: WriteChange[] = [];

beforeEach(async () => {
  h = await festivalHarness({ typeDefs: appTypeDefs, seed: [] });
  changes.length = 0;
  h.lora.onWrite((c) => changes.push(c));
  await h.data(`mutation {
    createUsers(input: [{ key: "u1", name: "Uma" }, { key: "u2", name: "Ugo" }]) { info { nodesCreated } }
  }`);
  changes.length = 0;
});

const count = async (cypher: string) =>
  Number((await h.db.execute(cypher)).rows[0]!["c"]);

describe("create", () => {
  test("nested create and connect, defaults, timestamps and generated keys", async () => {
    const d = await h.data<{
      createFestivals: {
        festivals: Array<Record<string, unknown>>;
        info: unknown;
      };
    }>(`
      mutation {
        createFestivals(input: [
          {
            name: "Sunland"
            genre: { create: { node: { key: "techno", name: "Techno" } } }
            followers: { connect: [{ key: "u1", edge: { since: 2020 } }, { key: "u2" }] }
          }
        ]) {
          festivals {
            key name capacity createdAt updatedAt
            genre { key name }
            followersConnection(sort: [{ key: ASC }]) { edges { properties { since } node { key } } }
          }
          info { nodesCreated relationshipsCreated nodesDeleted relationshipsDeleted }
        }
      }
    `);
    const [f] = d.createFestivals.festivals;
    expect(f).toMatchObject({
      name: "Sunland",
      capacity: 100,
      updatedAt: null,
      genre: { key: "techno", name: "Techno" },
      followersConnection: {
        edges: [
          { properties: { since: 2020 }, node: { key: "u1" } },
          { properties: { since: 2026 }, node: { key: "u2" } },
        ],
      },
    });
    expect(f!["key"]).toMatch(/^[0-9a-f-]{36}$/);
    expect(f!["createdAt"]).toMatch(/^\d{4}-\d\d-\d\dT/);
    expect(d.createFestivals.info).toEqual({
      nodesCreated: 2,
      relationshipsCreated: 3,
      nodesDeleted: 0,
      relationshipsDeleted: 0,
    });

    // The write-set is exact: every node touched, by type and key.
    expect(changes).toHaveLength(1);
    const c = changes[0]!;
    expect(c.created).toEqual([
      { type: "Festival", key: f!["key"] },
      { type: "Genre", key: "techno" },
    ]);
    expect(
      c.connected.map(
        (r) => `${r.field} ${String(r.from.key)}->${String(r.to.key)}`,
      ),
    ).toEqual([
      `Festival.genre ${String(f!["key"])}->techno`,
      `Festival.followers ${String(f!["key"])}->u1`,
      `Festival.followers ${String(f!["key"])}->u2`,
    ]);
    expect(c.types).toEqual(["Festival", "Genre", "User"]);
    expect(c.relationshipTypes).toEqual(["FOLLOWS", "IN_GENRE"]);
  });

  test("connecting a missing node writes nothing", async () => {
    const r = await h.run(`mutation {
      createFestivals(input: [{ name: "Ghost", followers: { connect: [{ key: "nobody" }] } }]) {
        festivals { key }
      }
    }`);
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("NOT_FOUND");
    expect(r.errors?.[0]?.message).toBe(
      'Festival.followers: no User with key "nobody"',
    );
    expect(await count("MATCH (f:Festival) RETURN count(f) AS c")).toBe(0);
    expect(changes).toEqual([]);
  });

  test("a duplicate key is a constraint violation naming the field", async () => {
    await h.data(
      `mutation { createUsers(input: [{ key: "u3" }]) { info { nodesCreated } } }`,
    );
    const r = await h.run(
      `mutation { createUsers(input: [{ key: "u3" }]) { info { nodesCreated } } }`,
    );
    expect(r.errors?.[0]?.extensions).toMatchObject({
      code: "CONSTRAINT_VIOLATION",
      type: "User",
      field: "key",
    });
    const dup = await h.run(
      `mutation { createUsers(input: [{ key: "u9" }, { key: "u9" }]) { info { nodesCreated } } }`,
    );
    expect(dup.errors?.[0]?.message).toBe('the input creates User "u9" twice');
  });

  test("a mutation creates at most maxBatch nodes", async () => {
    const input = Array.from(
      { length: 1001 },
      (_, i) => `{ key: "x${i}" }`,
    ).join(" ");
    const r = await h.run(
      `mutation { createUsers(input: [${input}]) { info { nodesCreated } } }`,
    );
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("LIMIT_EXCEEDED");
  });

  test("a single relationship stays single, whichever side writes it", async () => {
    await h.data(`mutation {
      createFestivals(input: [{ key: "f1", name: "One", genre: { create: { node: { key: "jazz", name: "Jazz" } } } }]) {
        info { nodesCreated }
      }
    }`);
    const r = await h.run(`mutation {
      createGenres(input: [{ key: "house", name: "House", festivals: { connect: [{ key: "f1" }] } }]) {
        info { nodesCreated }
      }
    }`);
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("CONSTRAINT_VIOLATION");
    expect(r.errors?.[0]?.message).toBe(
      'Festival.genre holds one Genre, but Festival "f1" would have 2',
    );
    expect(
      await count("MATCH (g:Genre {key: 'house'}) RETURN count(g) AS c"),
    ).toBe(0);
  });
});

describe("update and delete", () => {
  beforeEach(async () => {
    await h.data(`mutation {
      createFestivals(input: [{
        key: "f1", name: "One", capacity: 500,
        genre: { create: { node: { key: "jazz", name: "Jazz" } } },
        followers: { connect: [{ key: "u1" }, { key: "u2" }] }
      }]) { info { nodesCreated } }
    }`);
    await h.data(
      `mutation { createGenres(input: [{ key: "house", name: "House" }]) { info { nodesCreated } } }`,
    );
    changes.length = 0;
  });

  test("sets, removes with null, replaces a single relationship, disconnects", async () => {
    const d = await h.data<{ updateFestival: unknown }>(`mutation {
      updateFestival(key: "f1", update: {
        name: "Uno"
        capacity: null
        genre: { connect: { key: "house" } }
        followers: { disconnect: ["u2"] }
      }) {
        festival { name capacity updatedAt genre { key } followers { key } }
        info { relationshipsCreated relationshipsDeleted }
      }
    }`);
    expect(d.updateFestival).toMatchObject({
      festival: {
        name: "Uno",
        capacity: null,
        updatedAt: expect.stringMatching(/^\d{4}-/),
        genre: { key: "house" },
        followers: [{ key: "u1" }],
      },
      info: { relationshipsCreated: 1, relationshipsDeleted: 2 },
    });
    expect(changes[0]!.disconnected.map((r) => r.to.key).sort()).toEqual([
      "jazz",
      "u2",
    ]);
    expect(changes[0]!.updated).toEqual([{ type: "Festival", key: "f1" }]);
  });

  test("a required field cannot be set to null", async () => {
    const r = await h.run(
      `mutation { updateFestival(key: "f1", update: { name: null }) { info { nodesCreated } } }`,
    );
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("BAD_USER_INPUT");
  });

  test("updating a missing node returns null and writes nothing", async () => {
    const d = await h.data<{ updateFestival: unknown }>(
      `mutation { updateFestival(key: "nope", update: { name: "X" }) { festival { key } info { nodesCreated } } }`,
    );
    expect(d.updateFestival).toEqual({
      festival: null,
      info: { nodesCreated: 0 },
    });
    expect(changes).toHaveLength(1);
    expect(changes[0]!.updated).toEqual([]);
  });

  test("keys are immutable", async () => {
    const r = await h.run(
      `mutation { updateFestival(key: "f1", update: { key: "f2" }) { info { nodesCreated } } }`,
    );
    expect(r.errors?.[0]?.message).toMatch(
      unknownField("key", "FestivalUpdateInput"),
    );
  });

  test("delete reports counts and the neighbours in the write-set", async () => {
    const d = await h.data<{ deleteFestival: unknown }>(
      `mutation { deleteFestival(key: "f1") { nodesDeleted relationshipsDeleted } }`,
    );
    expect(d.deleteFestival).toEqual({
      nodesDeleted: 1,
      relationshipsDeleted: 3,
    });
    const c = changes[0]!;
    expect(c.deleted).toEqual([{ type: "Festival", key: "f1" }]);
    expect(c.entities.map((e) => `${e.type}:${String(e.key)}`).sort()).toEqual([
      "Festival:f1",
      "Genre:jazz",
      "User:u1",
      "User:u2",
    ]);
  });

  test("affects() matches reads to writes by label and relationship type", async () => {
    await h.data(`mutation { deleteFestival(key: "f1") { nodesDeleted } }`);
    const change = changes[0]!;
    const [genres] = h.lora.compile(`{ genres { key } }`);
    const [users] = h.lora.compile(`{ users { key follows { key } } }`);
    expect(h.lora.affects(genres!.compiled.reads, change)).toBe(true);
    expect(h.lora.affects(users!.compiled.reads, change)).toBe(true);
    const unrelated = { labels: ["Venue"], relationships: [] };
    expect(h.lora.affects(unrelated, change)).toBe(false);
  });
});

describe("@cypher", () => {
  beforeEach(async () => {
    await h.data(`mutation {
      createFestivals(input: [
        { key: "a", name: "Alpha", capacity: 3000, genre: { create: { node: { key: "rock", name: "Rock" } } },
          followers: { connect: [{ key: "u1" }, { key: "u2" }] } }
        { key: "b", name: "Beta", capacity: 1000, genre: { connect: { key: "rock" } } }
        { key: "c", name: "Gamma", capacity: 2000, genre: { connect: { key: "rock" } } }
      ]) { info { nodesCreated } }
    }`);
  });

  test("object fields: scalars and node lists, with arguments", async () => {
    const d = await h.data<{ festival: unknown }>(`{
      festival(key: "a") {
        followerCount
        similar { key followerCount }
        one: similar(limit: 1) { name }
      }
    }`);
    expect(d.festival).toEqual({
      followerCount: 2,
      similar: [
        { key: "b", followerCount: 0 },
        { key: "c", followerCount: 0 },
      ],
      one: [{ name: "Beta" }],
    });
  });

  test("root queries and mutations", async () => {
    const d = await h.data<Record<string, unknown>>(`{
      festivalCount
      biggestFestivals(min: 2000) { key genre { key } }
    }`);
    expect(d).toEqual({
      festivalCount: 3,
      biggestFestivals: [
        { key: "a", genre: { key: "rock" } },
        { key: "c", genre: { key: "rock" } },
      ],
    });
    changes.length = 0;
    const m = await h.data<{ renameGenre: unknown }>(
      `mutation { renameGenre(key: "rock", name: "Stone") { key name festivals(sort: [{ name: ASC }], limit: 1) { name } } }`,
    );
    expect(m.renameGenre).toEqual({
      key: "rock",
      name: "Stone",
      festivals: [{ name: "Alpha" }],
    });
    expect(changes[0]!.broad).toBe(true);
  });

  test("the generated statement wraps the hand-written one", () => {
    const [f] = h.lora.compile(
      `{ festival(key: "a") { similar(limit: 2) { key } } }`,
    );
    expect(f!.compiled.statements[0]!.text).toMatchInlineSnapshot(`
      "MATCH (this:Festival)
      WHERE this.key = $p0
      CALL {
        WITH this
        CALL {
          WITH this
          MATCH (this)-[:IN_GENRE]->(:Genre)<-[:IN_GENRE]-(other:Festival)
          WHERE other.key <> this.key
          RETURN other ORDER BY other.name LIMIT $p1
        }
        WITH other AS this_similar_node
        RETURN collect(this_similar_node { .key }) AS this_similar
      }
      RETURN this { similar: this_similar } AS this"
    `);
  });
});

describe("cost", () => {
  test("rejects an operation over the budget before it runs", async () => {
    const tight = await festivalHarness({
      typeDefs: appTypeDefs,
      seed: [],
      maxCost: 500,
    });
    const r = await tight.run(`{
      festivals(limit: 20) { key followers(limit: 20) { key follows(limit: 20) { key } } }
    }`);
    expect(r.errors?.[0]?.extensions).toMatchObject({
      code: "COST_EXCEEDED",
      cost: 8420,
      maxCost: 500,
    });
    expect(tight.statements).toEqual([]);
  });

  test("analyze() replaces page limits with measured degrees", async () => {
    const tight = await festivalHarness({
      typeDefs: appTypeDefs,
      seed: [],
      maxCost: 500,
    });
    await tight.data(`mutation {
      createFestivals(input: [{ key: "a", name: "A", followers: { connect: [] } }]) { info { nodesCreated } }
    }`);
    const stats = await tight.lora.analyze();
    expect(stats.nodes).toMatchObject({ Festival: 1 });
    expect(stats.degrees["Festival.followers"]).toMatchObject({
      max: 0,
      p99: 0,
    });
    const r = await tight.run(`{
      festivals(limit: 20) { key followers(limit: 20) { key follows(limit: 20) { key } } }
    }`);
    expect(r.errors).toBeUndefined();
  });
});

describe("upsert", () => {
  test("creates new keys, updates existing ones, in input order", async () => {
    await h.data(`mutation {
      createFestivals(input: [{ key: "f1", name: "One", capacity: 10 }]) { info { nodesCreated } }
    }`);
    changes.length = 0;
    const d = await h.data<{ upsertFestivals: unknown }>(`mutation {
      upsertFestivals(input: [
        { key: "f2", name: "Two" }
        { key: "f1", capacity: 20, followers: { connect: [{ key: "u1" }] } }
      ]) {
        festivals { key name capacity followers { key } }
        info { nodesCreated nodesUpdated relationshipsCreated }
      }
    }`);
    expect(d.upsertFestivals).toEqual({
      festivals: [
        { key: "f2", name: "Two", capacity: 100, followers: [] },
        { key: "f1", name: "One", capacity: 20, followers: [{ key: "u1" }] },
      ],
      info: { nodesCreated: 1, nodesUpdated: 1, relationshipsCreated: 1 },
    });
    expect(changes[0]).toMatchObject({
      operation: "UPSERT",
      created: [{ type: "Festival", key: "f2" }],
      updated: [{ type: "Festival", key: "f1" }],
    });
  });

  test("fields required on create are required for new keys only", async () => {
    const r = await h.run(
      `mutation { upsertFestivals(input: [{ key: "new" }]) { info { nodesCreated } } }`,
    );
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("BAD_USER_INPUT");
    expect(r.errors?.[0]?.message).toBe("Festival.name is required");
  });
});
