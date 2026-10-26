// `@cacheControl`: an operation's cache policy, on `execute()`'s result.

import { describe, expect, test } from "vitest";
import { createTestLoraGraphQL } from "../src/testing.js";

const typeDefs = /* GraphQL */ `
  type Album
    @node
    @cacheControl(maxAge: 600)
    @fulltext(indexes: [{ fields: ["title"] }]) {
    key: String! @key
    title: String!
    artist: Artist @relationship(type: "RELEASED", direction: IN)
    tracks: [Track!]! @relationship(type: "HAS_TRACK", direction: OUT)
    plays: Int @cacheControl(maxAge: 30)
  }
  type Artist @node @cacheControl(maxAge: 300) {
    key: String! @key
    name: String!
    viewerFollows: Boolean
      @cacheControl(maxAge: 0, scope: PRIVATE)
      @cypher(statement: "RETURN false AS f", columnName: "f")
  }
  type Track @node {
    key: String! @key
    title: String!
  }
  type Query {
    albumCount: Int!
      @cypher(
        statement: "MATCH (a:Album) RETURN count(a) AS n"
        columnName: "n"
      )
    hintedCount: Int!
      @cacheControl(maxAge: 60)
      @cypher(
        statement: "MATCH (a:Album) RETURN count(a) AS n"
        columnName: "n"
      )
  }
`;
const seed = [
  "CREATE (ar:Artist {key: 'a', name: 'Bicep'})-[:RELEASED]->(al:Album {key: 'isles', title: 'Isles', plays: 3})-[:HAS_TRACK]->(:Track {key: 't', title: 'Atlas'})",
];

describe("@cacheControl", () => {
  test("the smallest maxAge among the selected types and fields", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const policy = async (source: string) =>
      (await t.lora.execute({ source })).cacheControl;
    expect(await policy(`{ albums { key title } }`)).toEqual({
      maxAge: 600,
      scope: "PUBLIC",
    });
    expect(await policy(`{ albums { title artist { name } } }`)).toEqual({
      maxAge: 300,
      scope: "PUBLIC",
    });
    expect(await policy(`{ albums { title plays } }`)).toEqual({
      maxAge: 30,
      scope: "PUBLIC",
    });
    // A field's PRIVATE hint makes the whole result private.
    expect(
      await policy(`{ albums { artist { name viewerFollows } } }`),
    ).toEqual({ maxAge: 0, scope: "PRIVATE" });
  });

  test("generated wrappers take their type's hint", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const policy = async (source: string) =>
      (await t.lora.execute({ source })).cacheControl;
    const album = { maxAge: 600, scope: "PUBLIC" };
    expect(
      await policy(`{ searchAlbums(query: "isles") { score node { key } } }`),
    ).toEqual(album);
    expect(
      await policy(
        `{ searchAlbumsConnection(query: "isles") { totalCount edges { node { key } } pageInfo { hasNextPage } } }`,
      ),
    ).toEqual(album);
    expect(await policy(`{ albumsConnection { totalCount } }`)).toEqual(album);
  });

  test("no policy over an unhinted type, an unhinted scalar root, or a mutation", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const policy = async (source: string) =>
      (await t.lora.execute({ source })).cacheControl;
    expect(await policy(`{ albums { tracks { title } } }`)).toBeUndefined();
    expect(await policy(`{ tracks { key } }`)).toBeUndefined();
    expect(await policy(`{ albumCount }`)).toBeUndefined();
    expect(await policy(`{ albumCount albums { key } }`)).toBeUndefined();
    expect(await policy(`{ hintedCount albums { key } }`)).toEqual({
      maxAge: 60,
      scope: "PUBLIC",
    });
  });

  test("fragments count, the policy is never serialized, and the read-set stays", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const result = await t.lora.execute({
      source: `query Q { albums { ...Row } } fragment Row on Album { title artist { name } }`,
      readSet: true,
    });
    expect(result.cacheControl).toEqual({ maxAge: 300, scope: "PUBLIC" });
    expect(result.readSet?.labels).toContain("Album");
    const sent = JSON.parse(JSON.stringify(result)) as Record<string, unknown>;
    expect(Object.keys(sent).sort()).toEqual(["data", "extensions"]);
    expect(sent["data"]).toEqual({
      albums: [{ title: "Isles", artist: { name: "Bicep" } }],
    });
  });

  test("a negative maxAge is refused", async () => {
    await expect(
      createTestLoraGraphQL({
        typeDefs: `type A @node @cacheControl(maxAge: -1) { key: String! @key }`,
      }),
    ).rejects.toThrow(/maxAge must be 0 or more/);
  });
});
