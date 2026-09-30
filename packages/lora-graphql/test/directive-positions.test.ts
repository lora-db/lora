// Every library directive, in every kind of type and field: it either
// applies there or the model build refuses it by name. None is silently
// ignored.

import { parse } from "graphql";
import { describe, expect, test } from "vitest";
import { buildModel, ModelError } from "../src/index.js";
import { directiveTypeDefs } from "../src/model/directives.js";
import { DIRECTIVE_POSITIONS, type Position } from "../src/model/positions.js";

/** A valid use of each directive: arguments filled in. */
const USE: Record<string, string> = {
  node: "@node",
  key: "@key",
  declareRelationship: "@declareRelationship",
  groupBy: "@groupBy",
  customResolver: "@customResolver",
  unique: "@unique",
  index: "@index(kind: RANGE)",
  relationship: '@relationship(type: "X", direction: OUT)',
  settable: "@settable(onCreate: true)",
  selectable: "@selectable(onRead: true)",
  populatedBy: '@populatedBy(callback: "cb")',
  jwt: "@jwt",
  jwtClaim: '@jwtClaim(path: "x")',
  viewer: '@viewer(type: "T", field: "key")',
  relationshipProperties: "@relationshipProperties",
  alias: '@alias(property: "p")',
  private: "@private",
  cardinality: "@cardinality(max: 5)",
  query: "@query(read: true)",
  plural: '@plural(value: "Things")',
  filterable: "@filterable",
  sortable: "@sortable",
  limit: "@limit(max: 50)",
  relayId: "@relayId",
  fulltext: '@fulltext(indexes: [{ fields: ["key"] }])',
  vector: "@vector(dimensions: 3)",
  subscription: "@subscription",
  mutation: "@mutation",
  default: "@default(value: 1)",
  timestamp: "@timestamp",
  readonly: "@readonly",
  cypher: '@cypher(statement: "RETURN 1 AS x")',
  authentication: "@authentication",
  authorization:
    '@authorization(validate: [{ where: { jwt: { sub: { eq: "x" } } } }])',
};

const BASE = `type Claims @jwt { sub: String }
  type T implements I @node { key: String! @key  name: String
    friends: [T!]! @relationship(type: "K", direction: OUT, properties: "P") }
  type P @relationshipProperties { since: Int }
  interface I { key: String! }`;

/** A schema with `@SLOT` in the given position. */
const TEMPLATES: Record<Position, { at: "type" | "field"; sdl: string }> = {
  "node type": {
    at: "type",
    sdl: `type Claims @jwt { sub: String }  type N @node SLOT { key: String! @key }`,
  },
  "relationship properties type": {
    at: "type",
    sdl: BASE.replace(
      "@relationshipProperties",
      "@relationshipProperties SLOT",
    ),
  },
  "@jwt type": {
    at: "type",
    sdl: BASE.replace("type Claims @jwt", "type Claims @jwt SLOT"),
  },
  "object type without @node": {
    at: "type",
    sdl: `${BASE} type O SLOT { a: Int }
      type N @node { key: String! @key  o: O @cypher(statement: "RETURN {a: 1} AS o") }`,
  },
  "Query or Mutation type": {
    at: "type",
    sdl: `${BASE} type Query SLOT { q: Int @cypher(statement: "RETURN 1 AS q") }`,
  },
  interface: {
    at: "type",
    sdl: BASE.replace("interface I {", "interface I SLOT {"),
  },
  union: { at: "type", sdl: `${BASE} union U SLOT = T` },
  "node field": {
    at: "field",
    sdl: BASE.replace("name: String", "name: String SLOT"),
  },
  "relationship field": {
    at: "field",
    sdl: BASE.replace('properties: "P")', 'properties: "P") SLOT'),
  },
  "@cypher field": {
    at: "field",
    sdl: `${BASE} type N @node { key: String! @key  n: Int @cypher(statement: "RETURN 1 AS n") SLOT }`,
  },
  "Query or Mutation field": {
    at: "field",
    sdl: `${BASE} type Query { q: Int @cypher(statement: "RETURN 1 AS q") SLOT }`,
  },
  "@customResolver field": {
    at: "field",
    sdl: `${BASE} type N @node { key: String! @key  c: String @customResolver SLOT }`,
  },
  "relationship property": {
    at: "field",
    sdl: BASE.replace("since: Int", "since: Int SLOT"),
  },
  "interface field": {
    at: "field",
    sdl: BASE.replace(
      "interface I { key: String! }",
      "interface I { key: String!  name: String SLOT }",
    ),
  },
  "interface relationship field": {
    at: "field",
    sdl: BASE.replace(
      "interface I { key: String! }",
      "interface I { key: String!  friends: [T!]! @declareRelationship SLOT }",
    ),
  },
  "field of an object type without @node": {
    at: "field",
    sdl: `${BASE} type O { a: Int SLOT }
      type N @node { key: String! @key  o: O @cypher(statement: "RETURN {a: 1} AS o") }`,
  },
  "@jwt claim": {
    at: "field",
    sdl: BASE.replace(
      "type Claims @jwt { sub: String }",
      "type Claims @jwt { sub: String SLOT }",
    ),
  },
};

/**
 * Directives that decide the position themselves (`@node` makes a node
 * type, `@cypher` a computed field): used elsewhere they apply by moving
 * the definition to another position, or fail with a specific message.
 */
const SHAPING = new Set([
  "node",
  "relationshipProperties",
  "jwt",
  "customResolver",
  "cypher",
  "declareRelationship",
  "relationship",
]);

/** GraphQL locations each directive is declared for. */
const locations = new Map(
  parse(directiveTypeDefs).definitions.flatMap((d) =>
    d.kind === "DirectiveDefinition"
      ? [[d.name.value, d.locations.map((l) => l.value)] as const]
      : [],
  ),
);

const messages = (sdl: string): string[] => {
  try {
    buildModel(sdl);
    return [];
  } catch (err) {
    if (err instanceof ModelError) return err.problems.map((p) => p.message);
    throw err;
  }
};

describe("no directive is silently ignored", () => {
  test("every template builds without the slot", () => {
    for (const [position, { sdl }] of Object.entries(TEMPLATES)) {
      expect(
        messages(sdl.replace(" SLOT", "")),
        `template for ${position}`,
      ).toEqual([]);
    }
  });

  test("every library directive has a sample use", () => {
    expect(
      [...locations.keys()].filter((n) => !USE[n] && n !== "storedAs"),
    ).toEqual([]);
  });

  for (const [position, { at, sdl }] of Object.entries(TEMPLATES) as Array<
    [Position, (typeof TEMPLATES)[Position]]
  >) {
    test(`${position}: each directive applies or is refused by name`, () => {
      const graphqlLocations =
        at === "field"
          ? ["FIELD_DEFINITION"]
          : position === "interface"
            ? ["INTERFACE"]
            : position === "union"
              ? ["UNION"]
              : ["OBJECT"];
      for (const [name, use] of Object.entries(USE)) {
        if (!locations.get(name)!.some((l) => graphqlLocations.includes(l))) {
          continue; // graphql-js refuses it: wrong kind of definition.
        }
        const found = messages(sdl.replace("SLOT", use));
        const refused = found.some((m) =>
          m.startsWith(`@${name} is not supported on`),
        );
        if (DIRECTIVE_POSITIONS[position].includes(name)) {
          expect(refused, `@${name} on ${position}: ${found.join("; ")}`).toBe(
            false,
          );
        } else if (!SHAPING.has(name)) {
          expect(refused, `@${name} on ${position}: ${found.join("; ")}`).toBe(
            true,
          );
        }
      }
    });
  }

  test("an @authorization on an interface field is refused with a hint", () => {
    expect(
      messages(
        TEMPLATES["interface field"].sdl.replace("SLOT", USE["authorization"]!),
      ),
    ).toContain(
      "@authorization is not supported on an interface field; put it on the field of each implementation",
    );
  });
});
