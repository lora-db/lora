import { parse } from "graphql";
import { expect, test } from "vitest";
import { buildModel } from "../src/index.js";
import { migrateNeo4j } from "../src/migrate.js";

const neo4j = /* GraphQL */ `
  type Movie
    @node
    @fulltext(indexes: [{ indexName: "movieTitles", fields: ["title"] }]) {
    id: ID! @id @unique
    title: String!
    released: Int
    tagline: String @coalesce(value: "")
    actors: [Person!]!
      @relationship(
        type: "ACTED_IN"
        direction: IN
        queryDirection: DEFAULT_UNDIRECTED
        nestedOperations: [CONNECT, CONNECT_OR_CREATE]
      )
  }
  type Person @subscription(events: [CREATED, UPDATED, CREATE_RELATIONSHIP]) {
    id: ID! @id
    name: String!
    movies: [Movie!]! @relationship(type: "ACTED_IN", direction: OUT)
  }
`;

test("rewrites what maps and lists what does not", () => {
  const { typeDefs, todos } = migrateNeo4j(neo4j);
  expect(typeDefs).toContain("id: ID! @key(generate: true)");
  expect(typeDefs).toContain(
    '@fulltext(indexes: [{name: "movieTitles", fields: ["title"]}])',
  );
  expect(typeDefs).toContain(
    "queryDirection: UNDIRECTED, nestedOperations: [CONNECT]",
  );
  expect(typeDefs).toContain(
    "type Person @subscription(operations: [CREATE, UPDATE], relationships: true) @node @mutation",
  );
  expect(typeDefs).not.toContain("@coalesce");
  expect(todos).toEqual(
    expect.arrayContaining([
      "Movie.tagline: @coalesce is not supported: it hides predicates from indexes; store a value instead",
      "Movie.actors: connectOrCreate is not supported; use upsert",
      expect.stringContaining("Movie: @mutation added"),
    ]),
  );
  // The result is a valid model.
  expect(() => buildModel(typeDefs)).not.toThrow();
});

test("with operations, the opt-in surface follows observed usage", () => {
  const ops = [
    parse(
      `query { movies(where: { title_CONTAINS: "x", released: { gt: 2000 }, actors_SOME: { name_EQ: "Keanu" } }, sort: [{ title: ASC }]) { title } }`,
    ),
    parse(
      `mutation { createPeople(input: [{ id: "p", name: "n" }]) { info { nodesCreated } } }`,
    ),
  ];
  const { typeDefs, todos } = migrateNeo4j(neo4j, ops);
  expect(typeDefs).toContain(
    "title: String! @filterable(byValue: [CONTAINS]) @sortable",
  );
  expect(typeDefs).toContain("released: Int @filterable(byValue: [GT])");
  expect(typeDefs).toContain("name: String! @filterable(byValue: [EQ])");
  expect(typeDefs).toMatch(/type Person .*@mutation\(operations: \[CREATE\]\)/);
  expect(typeDefs).not.toMatch(/type Movie [^{]*@mutation/);
  expect(todos.some((t) => t.includes("@mutation added"))).toBe(false);
  expect(() => buildModel(typeDefs)).not.toThrow();
});
