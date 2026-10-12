// The single call must be invisible: the same mutation answers the same
// and leaves the same graph whether it went to the database as one call
// or through a transaction. Every case here runs both ways.

import { createDatabase } from "@loradb/lora-node";
import { describe, expect, test } from "vitest";
import { LoraGraphQL, loraDriver, type WriteChange } from "../src/index.js";

const typeDefs = /* GraphQL */ `
  extend schema @authorizationDefaults(requireAuthentication: false)

  type Doc
    @node
    @mutation
    @subscription(previousState: true)
    @authorization(
      filter: [{ where: { node: { owner: { eq: "$jwt.sub" } } } }]
      validate: [
        { operations: [CREATE], where: { node: { owner: { eq: "$jwt.sub" } } } }
        {
          operations: [UPDATE]
          when: [BEFORE]
          where: { node: { locked: { eq: false } } }
        }
        {
          operations: [UPDATE]
          when: [AFTER]
          where: { node: { open: { eq: true } } }
        }
      ]
    ) {
    key: String! @key
    owner: String
    title: String
    slug: String @unique
    locked: Boolean
    open: Boolean
    n: Int
    labels: [String!]
    stock: Int @range(min: 0)
    count: Int
      @authorization(
        validate: [
          { operations: [UPDATE], where: { jwt: { role: { eq: "admin" } } } }
        ]
      )
    boom: Int @cypher(statement: "RETURN 1 / this.n AS v")
    note: String
    stamp: String @populatedBy(callback: "stamp", operations: [CREATE, UPDATE])
    secret: String
      @authorization(
        validate: [
          { operations: [UPDATE], where: { jwt: { role: { eq: "admin" } } } }
        ]
      )
    tags: [Tag!]! @relationship(type: "HAS", direction: OUT)
  }

  type Tag @node @mutation {
    key: String! @key
  }
`;

const seed = [
  "CREATE (:Doc {key: 'mine', owner: 'me', title: 'One', slug: 'one', note: 'n', locked: false, open: true, n: 0})",
  "CREATE (:Doc {key: 'locked', owner: 'me', title: 'Two', locked: true, open: true})",
  "CREATE (:Doc {key: 'theirs', owner: 'other', title: 'Three', slug: 'three', locked: false, open: true})",
  "CREATE (:Tag {key: 't1'})",
];

interface Case {
  name: string;
  source: string;
  jwt?: Record<string, unknown>;
}

const me = { sub: "me" };

const cases: Case[] = [
  // Found in review.
  {
    name: "a field named in adjust with null still answers to its rule",
    source: `mutation { updateDoc(key: "mine", update: { title: "a" }, adjust: { count: null }) { doc { key title } } }`,
    jwt: me,
  },
  {
    name: "an update left out, on a missing node",
    source: `mutation { updateDoc(key: "nope") { doc { key } info { nodesUpdated } } }`,
    jwt: me,
  },
  {
    name: "an update left out, on a node",
    source: `mutation { updateDoc(key: "mine") { doc { key stamp } info { nodesUpdated } } }`,
    jwt: me,
  },
  {
    name: "an update left out, refused before",
    source: `mutation { updateDoc(key: "locked") { doc { key } } }`,
    jwt: me,
  },
  {
    name: "a rule refuses after the write and the payload read would fail too",
    source: `mutation { updateDoc(key: "mine", update: { open: false }) { doc { key boom } } }`,
    jwt: me,
  },
  {
    name: "a payload read that fails",
    source: `mutation { updateDoc(key: "mine", update: { title: "b" }) { doc { key boom } } }`,
    jwt: me,
  },
  {
    name: "adjust",
    source: `mutation { updateDoc(key: "mine", adjust: { n: { add: 5 } }) { doc { key n stamp } info { nodesUpdated } } }`,
    jwt: me,
  },
  {
    name: "adjust and set",
    source: `mutation { updateDoc(key: "mine", update: { title: "A" }, adjust: { n: { multiply: 3 } }) { doc { key n title } } }`,
    jwt: me,
  },
  {
    name: "adjust a missing node",
    source: `mutation { updateDoc(key: "nope", adjust: { n: { add: 1 } }) { doc { key } info { nodesUpdated } } }`,
    jwt: me,
  },
  {
    name: "adjust a field that is also set",
    source: `mutation { updateDoc(key: "mine", update: { n: 1 }, adjust: { n: { add: 1 } }) { doc { key n } } }`,
    jwt: me,
  },
  {
    name: "adjust a field that is also set, on a missing node",
    source: `mutation { updateDoc(key: "nope", update: { n: 1 }, adjust: { n: { add: 1 } }) { doc { key n } } }`,
    jwt: me,
  },
  {
    name: "adjust with two operations",
    source: `mutation { updateDoc(key: "mine", adjust: { n: { add: 1, subtract: 2 } }) { doc { key n } } }`,
    jwt: me,
  },
  {
    name: "adjust by dividing by zero",
    source: `mutation { updateDoc(key: "mine", adjust: { n: { divide: 0 } }) { doc { key n } } }`,
    jwt: me,
  },
  {
    name: "adjust a field the claims refuse",
    source: `mutation { updateDoc(key: "mine", adjust: { count: { add: 1 } }) { doc { key count } } }`,
    jwt: me,
  },
  {
    name: "adjust that field as admin",
    source: `mutation { updateDoc(key: "mine", adjust: { count: { add: 1 } }) { doc { key count } } }`,
    jwt: { sub: "me", role: "admin" },
  },
  {
    name: "adjust refused before",
    source: `mutation { updateDoc(key: "locked", adjust: { n: { add: 1 } }) { doc { key } } }`,
    jwt: me,
  },
  {
    name: "adjust a list",
    source: `mutation { updateDoc(key: "mine", adjust: { labels: { push: ["a", "b"] } }) { doc { key labels } } }`,
    jwt: me,
  },
  {
    name: "adjust a field with a constraint past it",
    source: `mutation { updateDoc(key: "mine", adjust: { stock: { subtract: 1 } }) { doc { key stock } } }`,
    jwt: me,
  },
  {
    name: "adjust a field with a constraint within it",
    source: `mutation { updateDoc(key: "mine", adjust: { stock: { add: 1 } }) { doc { key stock } } }`,
    jwt: me,
  },
  // Creates.
  {
    name: "create",
    source: `mutation { createDocs(input: [{ key: "n", owner: "me", title: "N" }]) { docs { key title stamp } info { nodesCreated } } }`,
    jwt: me,
  },
  {
    name: "create two shapes",
    source: `mutation { createDocs(input: [{ key: "n", owner: "me" }, { key: "m", owner: "me", title: "M" }]) { docs { key title } } }`,
    jwt: me,
  },
  {
    name: "create refused by a rule",
    source: `mutation { createDocs(input: [{ key: "n", owner: "other" }]) { docs { key } } }`,
    jwt: me,
  },
  {
    name: "create two shapes, one refused",
    source: `mutation { createDocs(input: [{ key: "n", owner: "me" }, { key: "m", owner: "other", title: "M" }]) { docs { key } } }`,
    jwt: me,
  },
  {
    name: "create under a hidden key",
    source: `mutation { createDocs(input: [{ key: "theirs", owner: "me" }]) { docs { key } } }`,
    jwt: me,
  },
  {
    name: "create under a visible key",
    source: `mutation { createDocs(input: [{ key: "mine", owner: "me" }]) { docs { key } } }`,
    jwt: me,
  },
  {
    name: "create with a taken unique value",
    source: `mutation { createDocs(input: [{ key: "n", owner: "me", slug: "three" }]) { docs { key } } }`,
    jwt: me,
  },
  {
    name: "create with a visible taken unique value",
    source: `mutation { createDocs(input: [{ key: "n", owner: "me", slug: "one" }]) { docs { key } } }`,
    jwt: me,
  },
  {
    name: "create under a hidden key that a rule also refuses",
    source: `mutation { createDocs(input: [{ key: "theirs", owner: "other" }]) { docs { key } } }`,
    jwt: me,
  },
  {
    name: "create anonymous",
    source: `mutation { createDocs(input: [{ key: "n" }]) { docs { key } } }`,
  },
  {
    name: "create nothing",
    source: `mutation { createDocs(input: []) { docs { key } info { nodesCreated } } }`,
    jwt: me,
  },
  {
    name: "create with a relationship",
    source: `mutation { createDocs(input: [{ key: "n", owner: "me", tags: { connect: [{ key: "t1" }] } }]) { docs { key tags { key } } } }`,
    jwt: me,
  },
  // Updates.
  {
    name: "update",
    source: `mutation { updateDoc(key: "mine", update: { title: "New" }) { doc { key title stamp } info { nodesUpdated } } }`,
    jwt: me,
  },
  {
    name: "update nothing",
    source: `mutation { updateDoc(key: "mine", update: {}) { doc { key title } info { nodesUpdated } } }`,
    jwt: me,
  },
  {
    name: "update to null",
    source: `mutation { updateDoc(key: "mine", update: { note: null }) { doc { key note } } }`,
    jwt: me,
  },
  {
    name: "update a missing node",
    source: `mutation { updateDoc(key: "nope", update: { title: "x" }) { doc { key } info { nodesUpdated } } }`,
    jwt: me,
  },
  {
    name: "update a node hidden from the caller",
    source: `mutation { updateDoc(key: "theirs", update: { title: "x" }) { doc { key } info { nodesUpdated } } }`,
    jwt: me,
  },
  {
    name: "update refused before",
    source: `mutation { updateDoc(key: "locked", update: { title: "x" }) { doc { key } } }`,
    jwt: me,
  },
  {
    name: "update refused after",
    source: `mutation { updateDoc(key: "mine", update: { open: false }) { doc { key } } }`,
    jwt: me,
  },
  {
    name: "update a field the claims refuse",
    source: `mutation { updateDoc(key: "mine", update: { secret: "s" }) { doc { key } } }`,
    jwt: me,
  },
  {
    name: "update that field as admin",
    source: `mutation { updateDoc(key: "mine", update: { secret: "s" }) { doc { key secret } } }`,
    jwt: { sub: "me", role: "admin" },
  },
  {
    name: "update that field on a missing node",
    source: `mutation { updateDoc(key: "nope", update: { secret: "s" }) { doc { key } } }`,
    jwt: me,
  },
  {
    name: "update to a taken unique value",
    source: `mutation { updateDoc(key: "mine", update: { slug: "three" }) { doc { key } } }`,
    jwt: me,
  },
  {
    name: "update with a relationship",
    source: `mutation { updateDoc(key: "mine", update: { title: "R", tags: { connect: [{ key: "t1" }] } }) { doc { title tags { key } } } }`,
    jwt: me,
  },
  {
    name: "update anonymous",
    source: `mutation { updateDoc(key: "mine", update: { title: "x" }) { doc { key } } }`,
  },
  {
    name: "update with only info",
    source: `mutation { updateDoc(key: "mine", update: { title: "I" }) { info { nodesUpdated } } }`,
    jwt: me,
  },
];

// Creates that connect: relationship rules, rules on both ends, single
// relationships and targets that are missing or hidden.
const linkTypeDefs = /* GraphQL */ `
  extend schema @authorizationDefaults(requireAuthentication: false)

  type Post
    @node
    @mutation
    @authorization(
      validate: [
        {
          operations: [CREATE_RELATIONSHIP]
          when: [BEFORE]
          where: { node: { open: { eq: true } } }
        }
        {
          operations: [CREATE_RELATIONSHIP]
          when: [AFTER]
          where: { node: { sealed: { eq: false } } }
        }
      ]
    ) {
    key: String! @key
    open: Boolean
    sealed: Boolean
    tags: [Tag!]!
      @relationship(type: "HAS", direction: OUT, properties: "Tagged")
      @authorization(
        validate: [
          {
            operations: [CONNECT]
            where: { target: { kind: { eq: "public" } } }
          }
          {
            operations: [UPDATE_EDGE]
            where: { jwt: { role: { eq: "admin" } } }
          }
        ]
      )
    cover: Image @relationship(type: "COVER", direction: OUT)
    comments: [Comment!]! @relationship(type: "ON", direction: IN)
    notes: [Note!]! @relationship(type: "NOTE", direction: OUT)
  }

  type Tagged @relationshipProperties {
    weight: Int
    source: String @default(value: "api")
  }

  type Tag
    @node
    @mutation
    @authorization(
      filter: [{ where: { node: { owner: { eq: "$jwt.sub" } } } }]
    ) {
    key: String! @key
    owner: String
    kind: String
  }

  type Image @node @mutation {
    key: String! @key
    post: Post @relationship(type: "COVER", direction: IN)
  }

  type Comment @node @mutation {
    key: String! @key
    post: Post! @relationship(type: "ON", direction: OUT)
  }

  type Note
    @node
    @mutation
    @authorization(
      validate: [
        { operations: [CREATE], where: { jwt: { role: { eq: "admin" } } } }
      ]
    ) {
    key: String! @key
  }
`;

const linkSeed = [
  "CREATE (:Tag {key: 't1', owner: 'me', kind: 'public'})",
  "CREATE (:Tag {key: 't2', owner: 'me', kind: 'private'})",
  "CREATE (:Tag {key: 't3', owner: 'other', kind: 'public'})",
  "CREATE (:Image {key: 'i1'})",
  "CREATE (:Post {key: 'p0', open: true, sealed: false})-[:COVER]->(:Image {key: 'i2'})",
  "MATCH (p:Post {key: 'p0'}) CREATE (:Comment {key: 'c0'})-[:ON]->(p)",
  "CREATE (:Post {key: 'shut', open: false, sealed: false})",
  "MATCH (p:Post {key: 'p0'}), (t:Tag {key: 't2'}) CREATE (p)-[:HAS {weight: 1}]->(t)",
];

const update = (key: string, fields: string, selection = "key") =>
  `mutation { updatePost(key: "${key}", update: { ${fields} }) { post { ${selection} } info { nodesUpdated nodesCreated relationshipsCreated } } }`;

const post = (fields: string, selection = "key") =>
  `mutation { createPosts(input: [{ key: "n", open: true, sealed: false, ${fields} }]) { posts { ${selection} } info { nodesCreated relationshipsCreated } } }`;

const linkCases = [
  {
    name: "connect",
    source: post(`tags: { connect: [{ key: "t1" }] }`, "key tags { key }"),
  },
  {
    name: "connect with a property",
    source: post(
      `tags: { connect: [{ key: "t1", edge: { weight: 3 } }] }`,
      "key tagsConnection { edges { properties { weight source } node { key } } }",
    ),
  },
  {
    name: "a nested create and a connect",
    source: post(
      `tags: { create: [{ node: { key: "new", owner: "me", kind: "public" } }], connect: [{ key: "t1" }] }`,
      "key tags(sort: [{ key: ASC }]) { key }",
    ),
  },
  {
    name: "connect the same node twice",
    source: post(
      `tags: { connect: [{ key: "t1" }, { key: "t1", edge: { weight: 2 } }] }`,
      "key tags { key }",
    ),
  },
  {
    name: "connect a missing node",
    source: post(`tags: { connect: [{ key: "nope" }] }`),
  },
  {
    name: "connect a hidden node",
    source: post(`tags: { connect: [{ key: "t3" }] }`),
  },
  {
    name: "connect a node, a missing one and a hidden one",
    source: post(
      `tags: { connect: [{ key: "nope" }, { key: "t1" }, { key: "t3" }] }`,
    ),
  },
  {
    name: "connect a node the relationship's rule refuses",
    source: post(`tags: { connect: [{ key: "t2" }] }`),
  },
  {
    name: "a nested create the relationship's rule refuses",
    source: post(
      `tags: { create: [{ node: { key: "new", owner: "me", kind: "private" } }] }`,
    ),
  },
  {
    name: "a nested create hidden from the caller",
    source: post(
      `tags: { create: [{ node: { key: "new", owner: "other", kind: "public" } }] }`,
      "key tags { key }",
    ),
  },
  {
    name: "connect refused on the owner before",
    source: `mutation { createPosts(input: [{ key: "n", open: false, sealed: false, tags: { connect: [{ key: "t1" }] } }]) { posts { key } } }`,
  },
  {
    name: "connect refused on the owner after",
    source: `mutation { createPosts(input: [{ key: "n", open: true, sealed: true, tags: { connect: [{ key: "t1" }] } }]) { posts { key } } }`,
  },
  {
    name: "connect refused before, to a missing node",
    source: `mutation { createPosts(input: [{ key: "n", open: false, sealed: false, tags: { connect: [{ key: "nope" }] } }]) { posts { key } } }`,
  },
  {
    name: "connect refused on the target",
    source: `mutation { createComments(input: [{ key: "c", post: { connect: { key: "shut" } } }]) { comments { key } } }`,
  },
  {
    name: "connect a required single relationship",
    source: `mutation { createComments(input: [{ key: "c", post: { connect: { key: "p0" } } }]) { comments { key post { key } } info { relationshipsCreated } } }`,
  },
  {
    name: "connect a required single relationship to a missing node",
    source: `mutation { createComments(input: [{ key: "c", post: { connect: { key: "nope" } } }]) { comments { key } } }`,
  },
  {
    name: "connect a single relationship",
    source: post(`cover: { connect: { key: "i1" } }`, "key cover { key }"),
  },
  {
    name: "connect a single relationship that is held",
    source: post(`cover: { connect: { key: "i2" } }`),
  },
  {
    name: "two creates connect one single relationship",
    source: `mutation { createPosts(input: [{ key: "n", open: true, sealed: false, cover: { connect: { key: "i1" } } }, { key: "m", open: true, sealed: false, cover: { connect: { key: "i1" } } }]) { posts { key } } }`,
  },
  {
    name: "connect from the other side of a single relationship that is held",
    source: `mutation { createImages(input: [{ key: "i9", post: { connect: { key: "p0" } } }]) { images { key } } }`,
  },
  {
    name: "a create under a taken key that connects",
    source: `mutation { createPosts(input: [{ key: "p0", open: true, sealed: false, tags: { connect: [{ key: "t1" }] } }]) { posts { key } } }`,
  },
  {
    name: "a nested create the claims refuse",
    source: post(`notes: { create: [{ node: { key: "x" } }] }`),
  },
  {
    name: "a nested create the claims refuse, behind a missing node",
    source: post(
      `tags: { connect: [{ key: "nope" }] }, notes: { create: [{ node: { key: "x" } }] }`,
    ),
  },
  {
    name: "a nested create the claims allow",
    source: post(
      `notes: { create: [{ node: { key: "x" } }] }`,
      "key notes { key }",
    ),
    jwt: { sub: "me", role: "admin" },
  },
  {
    name: "a nested create under a hidden key",
    source: post(
      `tags: { create: [{ node: { key: "t3", owner: "me", kind: "public" } }] }`,
      "key tags { key }",
    ),
  },
  // Updates that connect.
  {
    name: "update and connect",
    source: update(
      "p0",
      `sealed: false, tags: { connect: [{ key: "t1" }] }`,
      "key tags(sort: [{ key: ASC }]) { key }",
    ),
  },
  {
    name: "update with only a connect",
    source: update("p0", `tags: { connect: [{ key: "t1" }] }`),
  },
  {
    name: "update a missing node and connect",
    source: update("nope", `tags: { connect: [{ key: "t1" }] }`),
  },
  {
    name: "update a missing node and connect a missing node",
    source: update("nope", `tags: { connect: [{ key: "gone" }] }`),
  },
  {
    name: "update a missing node with a nested create",
    source: update(
      "nope",
      `tags: { create: [{ node: { key: "new", owner: "me", kind: "public" } }] }`,
    ),
  },
  {
    name: "update and connect a missing node",
    source: update("p0", `tags: { connect: [{ key: "gone" }] }`),
  },
  {
    name: "update and connect a hidden node",
    source: update("p0", `tags: { connect: [{ key: "t3" }] }`),
  },
  {
    name: "update and connect a node that is connected",
    source: update(
      "p0",
      `tags: { connect: [{ key: "t2" }] }`,
      "key tags { key }",
    ),
  },
  {
    name: "update and re-connect with a property, refused by the claims",
    source: update(
      "p0",
      `tags: { connect: [{ key: "t2", edge: { weight: 9 } }] }`,
    ),
  },
  {
    name: "update and re-connect with a property as admin",
    source: update(
      "p0",
      `tags: { connect: [{ key: "t2", edge: { weight: 9 } }] }`,
      "key tagsConnection { edges { properties { weight source } node { key } } }",
    ),
    jwt: { sub: "me", role: "admin" },
  },
  {
    name: "update, connect a new node and re-connect one",
    source: update(
      "p0",
      `tags: { connect: [{ key: "t1", edge: { weight: 4 } }, { key: "t2" }] }`,
      "key tagsConnection { edges { properties { weight source } node { key } } }",
    ),
  },
  {
    name: "update and connect a node the relationship's rule refuses",
    source: `mutation { updatePost(key: "shut", update: { open: true, tags: { connect: [{ key: "t2" }] } }) { post { key } } }`,
  },
  {
    name: "update and connect, refused on the owner before",
    source: update("shut", `tags: { connect: [{ key: "t1" }] }`),
  },
  {
    name: "update that opens the owner and connects",
    source: update(
      "shut",
      `open: true, tags: { connect: [{ key: "t1" }] }`,
      "key open tags { key }",
    ),
  },
  {
    name: "update and connect, refused on the owner after",
    source: update("p0", `sealed: true, tags: { connect: [{ key: "t1" }] }`),
  },
  {
    name: "update with a nested create and a connect",
    source: update(
      "p0",
      `tags: { create: [{ node: { key: "new", owner: "me", kind: "public" } }], connect: [{ key: "t1" }] }`,
      "key tags(sort: [{ key: ASC }]) { key }",
    ),
  },
  {
    name: "update with a nested create under a taken key",
    source: update(
      "p0",
      `tags: { create: [{ node: { key: "t1", owner: "me", kind: "public" } }] }`,
    ),
  },
  {
    name: "update with a nested create the claims refuse",
    source: update("p0", `notes: { create: [{ node: { key: "x" } }] }`),
  },
  {
    name: "update with a nested create the claims refuse, on a missing node",
    source: update("nope", `notes: { create: [{ node: { key: "x" } }] }`),
  },
  {
    name: "update and connect a comment that has a post",
    source: `mutation { updatePost(key: "shut", update: { open: true, comments: { connect: [{ key: "c0" }] } }) { post { key } } }`,
  },
  {
    name: "update and connect a free comment",
    source: update(
      "p0",
      `comments: { create: [{ node: { key: "c9" } }] }`,
      "key comments { key }",
    ),
  },
  {
    name: "update that replaces a single relationship",
    source: update(
      "p0",
      `cover: { connect: { key: "i1" } }`,
      "key cover { key }",
    ),
  },
  {
    name: "update that disconnects",
    source: update("p0", `tags: { disconnect: ["t2"] }`, "key tags { key }"),
  },
  {
    name: "update and connect anonymous",
    source: update("p0", `tags: { connect: [{ key: "t1" }] }`),
    anonymous: true,
  },
  {
    name: "a nested create under a visible key",
    source: post(
      `tags: { create: [{ node: { key: "t1", owner: "me", kind: "public" } }] }`,
    ),
  },
]
  .map(
    ({ anonymous, ...c }: Case & { anonymous?: boolean }): Case =>
      anonymous ? c : { jwt: me, ...c },
  )
  .concat({
    name: "connect anonymous",
    source: post(`tags: { connect: [{ key: "t1" }] }`),
  });

// @uniqueTogether over scalars and a relationship.
const uniqueTypeDefs = /* GraphQL */ `
  type Slot @node @mutation @uniqueTogether(fields: ["day", "stage"]) {
    key: String! @key
    day: String
    stage: Stage @relationship(type: "AT", direction: OUT)
  }

  type Stage @node @mutation {
    key: String! @key
    slots: [Slot!]! @relationship(type: "AT", direction: IN)
  }

  type Pair @node @mutation @uniqueTogether(fields: ["a", "b"]) {
    key: String! @key
    a: String
    b: String
  }
`;

const uniqueSeed = [
  "CREATE (s:Stage {key: 'main'}), (:Stage {key: 'side'}), (:Slot {key: 'fri', day: 'fri'})-[:AT]->(s)",
  "CREATE (:Pair {key: 'p1', a: 'x', b: 'y'}), (:Pair {key: 'p2', a: 'x', b: 'z'})",
];

const uniqueCases: Case[] = [
  {
    name: "create a free combination",
    source: `mutation { createPairs(input: [{ key: "n", a: "x", b: "w" }]) { pairs { key } } }`,
  },
  {
    name: "create a taken combination",
    source: `mutation { createPairs(input: [{ key: "n", a: "x", b: "y" }]) { pairs { key } } }`,
  },
  {
    name: "create two that share a combination",
    source: `mutation { createPairs(input: [{ key: "n", a: "q", b: "q" }, { key: "m", a: "q", b: "q" }]) { pairs { key } } }`,
  },
  {
    name: "create three, the last taken",
    source: `mutation { createPairs(input: [{ key: "n", a: "1", b: "1" }, { key: "m", a: "2", b: "2" }, { key: "o", a: "x", b: "z" }]) { pairs { key } } }`,
  },
  {
    name: "create with a null in the combination",
    source: `mutation { createPairs(input: [{ key: "n", a: "x" }]) { pairs { key } } }`,
  },
  {
    name: "update to a free combination",
    source: `mutation { updatePair(key: "p1", update: { b: "w" }) { pair { key b } } }`,
  },
  {
    name: "update to a taken combination",
    source: `mutation { updatePair(key: "p1", update: { b: "z" }) { pair { key b } } }`,
  },
  {
    name: "update a missing node",
    source: `mutation { updatePair(key: "nope", update: { b: "z" }) { pair { key } } }`,
  },
  {
    name: "create through a relationship, free",
    source: `mutation { createSlots(input: [{ key: "sat", day: "sat", stage: { connect: { key: "main" } } }]) { slots { key stage { key } } } }`,
  },
  {
    name: "create through a relationship, taken",
    source: `mutation { createSlots(input: [{ key: "again", day: "fri", stage: { connect: { key: "main" } } }]) { slots { key } } }`,
  },
  {
    name: "create through a relationship, same day on another stage",
    source: `mutation { createSlots(input: [{ key: "again", day: "fri", stage: { connect: { key: "side" } } }]) { slots { key } } }`,
  },
  {
    name: "create from the other side, taken",
    source: `mutation { createStages(input: [{ key: "new", slots: { create: [{ node: { key: "a", day: "d" } }, { node: { key: "b", day: "d" } }] } }]) { stages { key } } }`,
  },
];

const suites = [
  { suite: "scalars", typeDefs, seed, cases },
  { suite: "links", typeDefs: linkTypeDefs, seed: linkSeed, cases: linkCases },
  {
    suite: "unique together",
    typeDefs: uniqueTypeDefs,
    seed: uniqueSeed,
    cases: uniqueCases,
  },
];

async function outcome(
  single: boolean,
  c: Case,
  typeDefs: string,
  seed: string[],
) {
  let calls = 0;
  const db = await createDatabase();
  for (const statement of seed) await db.execute(statement);
  const lora = new LoraGraphQL({
    typeDefs,
    driver: loraDriver(db),
    singleCallMutations: single,
    maskErrors: false,
    callbacks: {
      stamp: ({ operation }) => {
        calls++;
        return `by-${operation}`;
      },
    },
  });
  await lora.assertSchema({ create: true });
  const changes: WriteChange[] = [];
  lora.onWrite((change) => changes.push(change));
  const result = await lora.execute({
    source: c.source,
    context: c.jwt ? { jwt: c.jwt } : {},
  });
  const graph = await db.execute(
    "MATCH (n) OPTIONAL MATCH (n)-[r]->(m) RETURN labels(n) AS labels, properties(n) AS props, type(r) AS rel, m.key AS target ORDER BY n.key, target",
  );
  return {
    data: result.data ?? null,
    errors: (result.errors ?? []).map((e) => {
      // A database error carries a random correlation id.
      const { id: _id, ...extensions } = e.extensions;
      return { message: e.message, extensions };
    }),
    graph: graph.rows,
    // How often a @populatedBy callback ran.
    calls,
    changes: changes.map(({ timestamp: _timestamp, ...change }) => change),
  };
}

describe.each(suites)(
  "one call and a transaction answer alike: $suite",
  ({ typeDefs, seed, cases }) => {
    test.each(cases)("$name", async (c) => {
      const [single, transaction] = await Promise.all([
        outcome(true, c, typeDefs, seed),
        outcome(false, c, typeDefs, seed),
      ]);
      expect(single).toEqual(transaction);
    });
  },
);
