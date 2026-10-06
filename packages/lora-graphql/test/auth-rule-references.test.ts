// `${node.path}` in a rule reads the rule's own node, so a rule can relate
// two of its paths: "the request's recipient is in the 1:1 it gates". In a
// relationship field's rules, `${source.path}`, `${target.path}` and
// `${edge.property}` read its ends and properties.

import { describe, expect, test } from "vitest";
import { buildModel, ModelError } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const codes = (r: { errors?: ReadonlyArray<{ extensions?: unknown }> }) =>
  r.errors?.map(
    (e) => (e.extensions as Record<string, unknown> | undefined)?.["code"],
  );
const claims = `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
}
`;

describe("${node.path}", () => {
  const typeDefs =
    claims +
    `type Person @node {
      key: String! @key
      requestsReceived: [Request!]! @relationship(type: "TO", direction: IN)
    }
    type Conversation @node @mutation
      @authorization(validate: [{ operations: [CREATE], where: { node: { participants: { some: { isViewer: true } } } } }]) {
      key: String! @key
      participants: [Person!]! @relationship(type: "IN", direction: IN)
    }
    type Request @node @mutation
      @authorization(validate: [{
        operations: [CREATE]
        where: { AND: [
          { node: { from: { isViewer: true } } }
          # Its recipient is in the conversation it gates.
          { node: { conversation: { participants: { some: { key: { eq: "\${node.to.key}" } } } } } }
        ] }
      }]) {
      key: String! @key
      from: Person! @relationship(type: "FROM", direction: IN)
      to: Person! @relationship(type: "TO", direction: OUT)
      conversation: Conversation! @relationship(type: "GATES", direction: OUT)
    }`;
  const seed = [
    "CREATE (:Person {key: 'lou'}), (:Person {key: 'bo'}), (:Person {key: 'sock'})",
    "MATCH (l:Person {key: 'lou'}), (b:Person {key: 'bo'}) CREATE (l)-[:IN]->(:Conversation {key: 'bo:lou'})<-[:IN]-(b)",
  ];
  const lou = { jwt: { sub: "lou", roles: [] } };
  const request = (key: string, to: string) =>
    `mutation { createRequests(input: [{ key: "${key}", from: { connect: { key: "lou" } }, to: { connect: { key: "${to}" } }, conversation: { connect: { key: "bo:lou" } } }]) { requests { key } } }`;

  test("relates two paths of the node being written", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    // A request to someone outside the 1:1 can't count as their consent.
    expect(codes(await t.run(request("r1", "sock"), {}, lou))).toEqual([
      "FORBIDDEN",
    ]);
    expect(codes(await t.run(request("r2", "bo"), {}, lou))).toBeUndefined();
  });

  test("reads a scalar of the node itself, inside a longer string", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs:
        claims +
        `type Person @node { key: String! @key }
        type Pair @node @mutation
          @authorization(validate: [{ operations: [CREATE], where: { AND: [
            { node: { a: { isViewer: true } } }
            # Keyed by its two people, in order: one per pair.
            { node: { key: { eq: "\${node.a.key}:\${node.b.key}" } } }
          ] } }]) {
          key: String! @key
          a: Person! @relationship(type: "A", direction: IN)
          b: Person! @relationship(type: "B", direction: IN)
        }`,
      seed: ["CREATE (:Person {key: 'lou'}), (:Person {key: 'bo'})"],
    });
    const pair = (key: string) =>
      `mutation { createPairs(input: [{ key: "${key}", a: { connect: { key: "lou" } }, b: { connect: { key: "bo" } } }]) { pairs { key } } }`;
    expect(codes(await t.run(pair("lou:sock"), {}, lou))).toEqual([
      "FORBIDDEN",
    ]);
    expect(codes(await t.run(pair("lou:bo"), {}, lou))).toBeUndefined();
  });

  test("works in filter rules: a row is visible by a relation between its own paths", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: `extend schema @authorizationDefaults(requireAuthentication: false)
type Person @node { key: String! @key }
        type Project @node {
          key: String! @key
          members: [Person!]! @relationship(type: "MEMBER", direction: IN)
        }
        type Task @node
          @authorization(filter: [{ where: { node: { project: { members: { some: { key: { eq: "\${node.assignee.key}" } } } } } } }]) {
          key: String! @key
          project: Project! @relationship(type: "OF", direction: OUT)
          assignee: Person! @relationship(type: "ASSIGNED", direction: IN)
        }`,
      seed: [
        "CREATE (a:Person {key: 'a'}), (b:Person {key: 'b'}), (p:Project {key: 'p'}), (a)-[:MEMBER]->(p)",
        "MATCH (a:Person {key: 'a'}), (b:Person {key: 'b'}), (p:Project {key: 'p'}) CREATE (a)-[:ASSIGNED]->(:Task {key: 'on'})-[:OF]->(p), (b)-[:ASSIGNED]->(:Task {key: 'off'})-[:OF]->(p)",
      ],
    });
    const r = await t.data<{ tasks: Array<{ key: string }> }>(
      "{ tasks { key } }",
    );
    expect(r.tasks.map((x) => x.key)).toEqual(["on"]);
  });
});

describe("${source.path} / ${target.path} in relationship rules", () => {
  test("a payer must be on the expense's trip", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs:
        claims +
        `type Person @node { key: String! @key }
        type Trip @node {
          key: String! @key
          members: [Person!]! @relationship(type: "ON", direction: IN)
        }
        type Expense @node @mutation @authorization(public: [CREATE, UPDATE, DELETE]) {
          key: String! @key
          trip: Trip! @relationship(type: "OF", direction: OUT)
          paidBy: Person!
            @relationship(type: "PAID", direction: IN)
            @authorization(validate: [{ operations: [CONNECT], where: { source: { trip: { members: { some: { key: { eq: "\${target.key}" } } } } } } }])
        }`,
      seed: [
        "CREATE (lou:Person {key: 'lou'}), (:Person {key: 'stranger'}), (t:Trip {key: 't'}), (lou)-[:ON]->(t)",
      ],
    });
    const expense = (key: string, payer: string) =>
      `mutation { createExpenses(input: [{ key: "${key}", trip: { connect: { key: "t" } }, paidBy: { connect: { key: "${payer}" } } }]) { expenses { key } } }`;
    const lou = { jwt: { sub: "lou", roles: [] } };
    expect(codes(await t.run(expense("e1", "stranger"), {}, lou))).toEqual([
      "FORBIDDEN",
    ]);
    expect(codes(await t.run(expense("e2", "lou"), {}, lou))).toBeUndefined();
  });
});

describe("model checks", () => {
  const build =
    (rule: string, extra = "") =>
    () =>
      buildModel(
        claims +
          `type Person @node { key: String! @key  pets: [Pet!]! @relationship(type: "OWNS", direction: OUT) }
        type Pet @node { key: String! @key }
        type Thing @node @mutation @authorization(validate: [{ operations: [CREATE], where: ${rule} }]) {
          key: String! @key
          name: String
          owner: Person! @relationship(type: "HAS", direction: IN)
          friends: [Person!]! @relationship(type: "FRIEND", direction: IN)
        }
        ${extra}`,
      );

  test("a reference must reach a scalar through single relationships", () => {
    expect(
      build(`{ node: { name: { eq: "\${node.owner.key}" } } }`),
    ).not.toThrow();
    expect(build(`{ node: { name: { eq: "\${node.nope}" } } }`)).toThrow(
      /Thing\.nope is not a scalar field/,
    );
    expect(build(`{ node: { name: { eq: "\${node.owner}" } } }`)).toThrow(
      /Thing\.owner is not a scalar field/,
    );
    expect(build(`{ node: { name: { eq: "\${node.friends.key}" } } }`)).toThrow(
      /Thing\.friends is not a single relationship/,
    );
    expect(
      build(`{ node: { name: { eq: "\${node.owner.pets.key}" } } }`),
    ).toThrow(/Person\.pets is not a single relationship/);
  });

  test("it stands for one value, where its node is known", () => {
    expect(build(`{ node: { name: { in: ["\${node.key}"] } } }`)).toThrow(
      /stands for one value, not inside a list/,
    );
    expect(build(`{ node: { name: { eq: "\${source.key}" } } }`)).toThrow(
      /reads a relationship rule's ends/,
    );
    expect(build(`{ viewer: { key: { eq: "\${node.key}" } } }`)).toThrow(
      /viewer: \{ … \} tests the caller's node alone/,
    );
    expect(
      build(
        '{ node: { key: { eq: "x" } } }',
        `type Box @node @mutation {
          key: String! @key
          item: Thing! @relationship(type: "IN", direction: IN)
            @authorization(validate: [{ operations: [CONNECT], where: { source: { key: { eq: "\${node.key}" } } } }])
        }`,
      ),
    ).toThrow(/a relationship rule reads its ends/);
  });

  test("a named rule reading ${node.…} can't stand inside another node's filter", () => {
    expect(() =>
      buildModel(
        claims +
          `type Person @node { key: String! @key }
          type Trip @node
            @authorizationRule(name: "selfOwned", where: { node: { owner: { key: { eq: "\${node.key}" } } } }) {
            key: String! @key
            owner: Person! @relationship(type: "OWNS", direction: IN)
          }
          type Stop @node @mutation @authorization(validate: [{ operations: [CREATE], where: { node: { trip: { rule: "selfOwned" } } } }]) {
            key: String! @key
            trip: Trip! @relationship(type: "OF", direction: OUT)
          }`,
      ),
    ).toThrow(ModelError);
  });
});
