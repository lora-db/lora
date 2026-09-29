// LoraDB 0.15 ignores end-node labels in MATCH patterns; every expansion
// the library writes must still stay typed. FOLLOWS here links a User to
// both Users and Artists.

import { festivalHarness } from "./harness.js";

const typeDefs = /* GraphQL */ `
  type User @node @mutation {
    key: String! @key
    name: String @sortable
    follows: [Artist!]!
      @relationship(type: "FOLLOWS", direction: OUT)
      @filterable
    friends: [User!]! @relationship(type: "FOLLOWS", direction: OUT)
  }
  type Artist @node @mutation {
    key: String! @key
    name: String @sortable
    fans: [User!]! @relationship(type: "FOLLOWS", direction: IN) @filterable
  }
`;

const seed = [
  `CREATE (u:User {key: 'u1', name: 'Uma'}), (v:User {key: 'u2', name: 'Vic'}), (a:Artist {key: 'a1', name: 'Ann'})
   CREATE (u)-[:FOLLOWS]->(v), (u)-[:FOLLOWS]->(a), (v)-[:FOLLOWS]->(a)`,
];

test("expansions only return nodes of the field's type", async () => {
  const h = await festivalHarness({ typeDefs, seed });
  const d = await h.data<Record<string, unknown>>(`{
    user(key: "u1") {
      sorted: follows(sort: [{ name: ASC }]) { key }
      follows { key }
      friends(sort: [{ name: ASC }]) { key }
      followsConnection { totalCount edges { node { key } } }
    }
    artists(where: { fans: { some: { key: { eq: "u1" } } } }) { key }
    users(where: { follows: { some: { key: { eq: "a1" } } } }, sort: [{ key: ASC }]) { key }
  }`);
  expect(d).toEqual({
    user: {
      sorted: [{ key: "a1" }],
      follows: [{ key: "a1" }],
      friends: [{ key: "u2" }],
      followsConnection: { totalCount: 1, edges: [{ node: { key: "a1" } }] },
    },
    artists: [{ key: "a1" }],
    users: [{ key: "u1" }, { key: "u2" }],
  });
  const r = await h.data<{ updateUser: unknown }>(
    `mutation { updateUser(key: "u1", update: { follows: { disconnect: ["u2", "a1"] } }) { info { relationshipsDeleted } } }`,
  );
  expect(r.updateUser).toEqual({ info: { relationshipsDeleted: 1 } });
});
