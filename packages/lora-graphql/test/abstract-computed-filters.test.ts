// A @filterable @cypher scalar filtered through an interface root: each
// implementation computes it per candidate, as its own root list would.

import { describe, expect, test } from "vitest";
import { buildModel, ModelError } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const pathText = `pathText: String @filterable(byValue: [CONTAINS, EQ])
    @cypher(statement: "RETURN reduce(s = '', p IN this.paths | s + '/' + p) AS t", columnName: "t", cost: 1)`;
const typeDefs = `interface MediaFile {
  key: String!
  pathText: String @filterable(byValue: [CONTAINS, EQ])
}
type Image implements MediaFile @node {
  key: String! @key
  paths: [String!]!
  ${pathText}
}
type Video implements MediaFile @node {
  key: String! @key
  paths: [String!]!
  ${pathText}
}`;
const seed =
  "CREATE (:Image {key: 'i1', paths: ['a', 'cat']}), (:Video {key: 'v1', paths: ['b', 'dog']}), (:Image {key: 'i2', paths: ['dog']})";

function problems(sdl: string): string[] {
  try {
    buildModel(sdl);
  } catch (err) {
    if (err instanceof ModelError) {
      return err.problems.map(
        (p) => `${p.type}${p.field ? `.${p.field}` : ""}: ${p.message}`,
      );
    }
    throw err;
  }
  return [];
}

describe("a computed field filtered through an interface root", () => {
  test("filters the list and the connection, totalCount included", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    expect(
      await t.data(
        '{ mediaFiles(where: { pathText: { contains: "dog" } }) { __typename key } }',
      ),
    ).toEqual({
      mediaFiles: [
        { __typename: "Image", key: "i2" },
        { __typename: "Video", key: "v1" },
      ],
    });
    expect(
      await t.data(
        '{ mediaFilesConnection(where: { OR: [{ pathText: { eq: "/a/cat" } }, { pathText: { contains: "b/" } }] }) { totalCount edges { node { key } } } }',
      ),
    ).toEqual({
      mediaFilesConnection: {
        totalCount: 2,
        edges: [{ node: { key: "i1" } }, { node: { key: "v1" } }],
      },
    });
  });

  test("an implementation must declare the filters itself", () => {
    expect(
      problems(
        typeDefs.replace(
          /(type Video[\s\S]*?)pathText: String @filterable\(byValue: \[CONTAINS, EQ\]\)/,
          "$1pathText: String",
        ),
      ),
    ).toEqual([
      "MediaFile.pathText: Video.pathText is a @cypher field: give it @filterable for CONTAINS, EQ too, so its statement runs for the interface's filter",
    ]);
  });

  test("an interface root cannot sort by it", () => {
    expect(
      problems(
        typeDefs.replace(
          "  pathText: String @filterable(byValue: [CONTAINS, EQ])\n}",
          "  pathText: String @filterable(byValue: [CONTAINS, EQ]) @sortable\n}",
        ),
      ),
    ).toEqual(
      expect.arrayContaining([
        "MediaFile.pathText: Image.pathText is a @cypher field: an interface root cannot sort by it; sort the implementation's own root",
      ]),
    );
  });
});
