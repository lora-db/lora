import {
  buildModel,
  inferRequirements,
  ModelError,
  requirementDdl,
} from "../src/index.js";
import { festivalTypeDefs } from "./fixtures.js";

function problems(typeDefs: string): string[] {
  try {
    buildModel(typeDefs);
  } catch (err) {
    if (err instanceof ModelError) {
      return err.problems.map((p) =>
        [p.type && p.field ? `${p.type}.${p.field}` : p.type, p.message]
          .filter(Boolean)
          .join(": "),
      );
    }
    throw err;
  }
  return [];
}

describe("model validation", () => {
  test("the fixture model is valid", () => {
    const model = buildModel(festivalTypeDefs);
    expect([...model.nodes.keys()]).toEqual([
      "Festival",
      "Genre",
      "User",
      "Artist",
    ]);
    const festival = model.nodes.get("Festival")!;
    expect(festival.plural).toBe("festivals");
    expect(festival.key.name).toBe("key");
    expect(festival.limit).toEqual({ default: 10, max: 50 });
    expect(festival.fields.get("title")).toMatchObject({
      property: "displayTitle",
    });
  });

  test("reports every problem at once, located by type and field", () => {
    expect(
      problems(`
        type A @node {
          name: String
          b: B
          c: [C!]! @relationship(type: "has-c", direction: OUT)
          tags: [String]
          n: Int @filterable(byValue: [CONTAINS])
          p: Point @sortable
        }
        type B @node { key: Float @key }
        type C @node { k1: ID! @key k2: ID! @key }
        type D { x: Int }
      `),
    ).toEqual([
      "D: object types need @node or @relationshipProperties",
      "A.b: B is a @node type; add @relationship(type:, direction:)",
      "A.c: relationship type `has-c` must be SCREAMING_SNAKE_CASE",
      "A.tags: list items must be non-null, e.g. [String!]",
      "A.n: Int does not support CONTAINS (allowed: EQ, IN, LT, LTE, GT, GTE)",
      "A.p: Point cannot be @sortable",
      "A: a @node type needs exactly one @key field",
      "B.key: @key must be String, ID, Int or BigInt",
      "B.key: @key fields must be non-null",
      "C: only one @key field is allowed (found k1, k2)",
    ]);
  });

  test("graphql-js validates directive usage", () => {
    expect(
      problems(`type A @node { key: ID! @key @filterable(byValue: [NOPE]) }`),
    ).toEqual([
      'A.key: @filterable: Argument "byValue" has invalid value [NOPE].',
    ]);
    expect(problems(`type A @node { key: ID! @key @unknown }`)[0]).toMatch(
      /Unknown directive/,
    );
  });

  test("relationship rules", () => {
    expect(
      problems(`
        type A @node {
          key: ID! @key
          bs: [B] @relationship(type: "HAS", direction: OUT)
          c: B @relationship(type: "HAS", direction: OUT, properties: "Nope") @cardinality(max: 3)
          d: B @relationship(type: "HAS", direction: OUT) @filterable(byValue: [EQ])
          e: Int @relationship(type: "HAS", direction: OUT)
        }
        type B @node { key: ID! @key }
      `),
    ).toEqual([
      "A.bs: relationship lists must be written [T!]!",
      "A.c: Nope is not a @relationshipProperties type",
      "A.c: @cardinality only applies to list relationships",
      "A.d: @filterable on a relationship takes no byValue",
      "A.e: @relationship target must be a @node type",
    ]);
  });

  test("names, aliases and limits", () => {
    expect(
      problems(`
        type Person @node(plural: "people") @limit(default: 500, max: 1000) {
          key: ID! @key @relayId
          id: String
          a: String
          b: String @alias(property: "a")
          secret: String @private @filterable
        }
        type Persons @node(labels: ["Person"]) { key: ID! @key }
        type Query { hello: String }
      `),
    ).toEqual([
      "Person.secret: @private fields cannot be @filterable or @sortable",
      "Person.b: stores property `a`, already stored by a",
      "Person.id: @relayId adds a global `id` field; rename this field or map it with @alias",
      "Person: @limit(max: 1000) exceeds the global maximum 100",
      "Persons: primary label `Person` is already used by Person",
      "Query.hello: Query fields are generated; a custom one needs @cypher",
    ]);
  });

  test("root field collisions", () => {
    expect(
      problems(`
        type Series @node(plural: "series") { key: ID! @key }
      `),
    ).toEqual([
      "Series: root field `series` collides with Series; set @node(plural:)",
    ]);
  });

  test("temporal RANGE indexes are refused until the engine supports them", () => {
    expect(
      problems(
        `type A @node { key: ID! @key at: DateTime @index(kind: RANGE) }`,
      ),
    ).toEqual([
      "A.at: LoraDB cannot RANGE-index DateTime values yet: range filters through such an index return no rows",
    ]);
  });

  test("the key is always addressable by value", () => {
    const model = buildModel(`type A @node { key: ID! @key }`);
    expect([...model.nodes.get("A")!.key.filters]).toEqual(["EQ", "IN"]);
  });
});

describe("S1: index inference", () => {
  test("derives constraints and indexes from the API", () => {
    const reqs = inferRequirements(buildModel(festivalTypeDefs)).map(
      (r) =>
        `${r.kind === "index" ? r.index : r.constraint} ${r.label}.${r.property} — ${r.reason}`,
    );
    expect(reqs).toEqual([
      "NODE_KEY Festival.key — Festival.key is the @key",
      "NOT_NULL Festival.name — Festival.name is non-null and @sortable",
      "RANGE Festival.name — Festival.name is @sortable",
      "TEXT Festival.name — Festival.name is @filterable by CONTAINS, STARTS_WITH",
      "RANGE Festival.capacity — Festival.capacity is @filterable by LT, GT, GTE",
      "NODE_KEY Genre.key — Genre.key is the @key",
      "NOT_NULL Genre.name — Genre.name is non-null and @sortable",
      "RANGE Genre.name — Genre.name is @sortable",
      "TEXT Genre.name — Genre.name is @filterable by STARTS_WITH",
      "NODE_KEY User.key — User.key is the @key",
      "RANGE User.name — User.name is @sortable",
      "NODE_KEY Artist.key — Artist.key is the @key",
      "NOT_NULL Artist.name — Artist.name is non-null and @sortable",
      "RANGE Artist.name — Artist.name is @sortable",
      "TEXT Artist.name — Artist.name is @filterable by CONTAINS",
    ]);
  });

  test("EQ / IN need no index; @unique covers RANGE; @index adds", () => {
    const reqs = inferRequirements(
      buildModel(`
        type A @node {
          key: ID! @key
          slug: String @unique @sortable
          city: String @filterable(byValue: [EQ, IN])
          loc: Point @index(kind: POINT)
        }
      `),
    );
    expect(reqs.map((r) => requirementDdl(r))).toEqual([
      "CREATE CONSTRAINT `a_key_key` IF NOT EXISTS FOR (n:`A`) REQUIRE n.`key` IS NODE KEY",
      "CREATE CONSTRAINT `a_slug_unique` IF NOT EXISTS FOR (n:`A`) REQUIRE n.`slug` IS UNIQUE",
      "CREATE POINT INDEX `a_loc_point` IF NOT EXISTS FOR (n:`A`) ON (n.`loc`)",
    ]);
  });
});
