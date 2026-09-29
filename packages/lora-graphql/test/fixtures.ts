export const festivalTypeDefs = /* GraphQL */ `
  enum Status {
    ANNOUNCED
    ON_SALE
    SOLD_OUT
  }

  type Festival @node @query(aggregate: true) @limit(default: 10, max: 50) {
    key: String! @key @relayId
    name: String!
      @filterable(byValue: [EQ, IN, CONTAINS, STARTS_WITH])
      @sortable
    capacity: Int @filterable(byValue: [EQ, LT, GT, GTE]) @sortable
    status: Status @filterable
    startsAt: DateTime @filterable(byValue: [GTE, LT]) @sortable
    internalNotes: String @private
    title: String @alias(property: "displayTitle")
    genre: Genre @relationship(type: "IN_GENRE", direction: OUT) @filterable
    followers: [User!]!
      @relationship(type: "FOLLOWS", direction: IN, properties: "Follows")
      @filterable
    headliners: [Artist!]! @relationship(type: "HEADLINES", direction: IN)
  }

  type Genre @node {
    key: String! @key
    name: String! @filterable(byValue: [EQ, IN, STARTS_WITH]) @sortable
    festivals: [Festival!]! @relationship(type: "IN_GENRE", direction: IN)
  }

  type User @node {
    key: String! @key
    name: String @sortable @filterable
    follows: [Festival!]!
      @relationship(type: "FOLLOWS", direction: OUT, properties: "Follows")
  }

  type Artist @node {
    key: ID! @key
    name: String! @sortable @filterable(byValue: [EQ, CONTAINS])
    plays: [Festival!]! @relationship(type: "HEADLINES", direction: OUT)
  }

  type Follows @relationshipProperties {
    since: Int! @filterable(byValue: [EQ, GTE, LT])
  }
`;

export const seedStatements = [
  `UNWIND range(0, 29) AS i
   CREATE (:Festival {
     key: 'f' + toString(i),
     name: CASE i % 3 WHEN 0 THEN 'Sunland ' WHEN 1 THEN 'Moonfest ' ELSE 'Starland ' END + toString(i),
     capacity: CASE WHEN i % 5 = 0 THEN null ELSE 1000 * i END,
     status: CASE i % 3 WHEN 0 THEN 'ANNOUNCED' WHEN 1 THEN 'ON_SALE' ELSE 'SOLD_OUT' END,
     displayTitle: 'The ' + toString(i),
     internalNotes: 'secret'
   })`,
  `UNWIND ['techno', 'house', 'jazz'] AS g CREATE (:Genre {key: g, name: toUpper(substring(g, 0, 1)) + substring(g, 1)})`,
  `MATCH (f:Festival), (g:Genre)
   WHERE (toInteger(substring(f.key, 1)) % 3 = 0 AND g.key = 'techno')
      OR (toInteger(substring(f.key, 1)) % 3 = 1 AND g.key = 'house')
   CREATE (f)-[:IN_GENRE]->(g)`,
  `UNWIND range(0, 9) AS i CREATE (:User {key: 'u' + toString(i), name: 'User ' + toString(i)})`,
  `MATCH (u:User), (f:Festival)
   WHERE toInteger(substring(f.key, 1)) % 10 = toInteger(substring(u.key, 1))
      OR (u.key = 'u0' AND toInteger(substring(f.key, 1)) < 5)
   CREATE (u)-[:FOLLOWS {since: 2000 + toInteger(substring(f.key, 1))}]->(f)`,
  `UNWIND range(0, 4) AS i CREATE (:Artist {key: 'a' + toString(i), name: 'Artist ' + toString(i)})`,
  `MATCH (a:Artist), (f:Festival)
   WHERE toInteger(substring(f.key, 1)) % 5 = toInteger(substring(a.key, 1))
   CREATE (a)-[:HEADLINES]->(f)`,
];

/** Mutations, @cypher fields and custom roots. */
export const appTypeDefs = /* GraphQL */ `
  type Festival @node @mutation @query(aggregate: true) {
    key: ID! @key(generate: true)
    name: String! @filterable(byValue: [EQ, CONTAINS]) @sortable
    capacity: Int @filterable(byValue: [GT, LT]) @default(value: 100)
    createdAt: DateTime @timestamp(operations: [CREATE])
    updatedAt: DateTime @timestamp(operations: [UPDATE])
    genre: Genre @relationship(type: "IN_GENRE", direction: OUT)
    followers: [User!]!
      @relationship(type: "FOLLOWS", direction: IN, properties: "Follows")
      @filterable
    followerCount: Int!
      @cypher(statement: "RETURN size([(this)<-[:FOLLOWS]-(:User) | 1]) AS n")
    similar(limit: Int = 3): [Festival!]!
      @cypher(
        statement: """
        MATCH (this)-[:IN_GENRE]->(:Genre)<-[:IN_GENRE]-(other:Festival)
        WHERE other.key <> this.key
        RETURN other ORDER BY other.name LIMIT $limit
        """
        columnName: "other"
      )
  }

  type Genre @node @mutation(operations: [CREATE]) {
    key: String! @key
    name: String!
    festivals: [Festival!]! @relationship(type: "IN_GENRE", direction: IN)
  }

  type User @node @mutation {
    key: String! @key
    name: String @sortable
    follows: [Festival!]!
      @relationship(type: "FOLLOWS", direction: OUT, properties: "Follows")
  }

  type Follows @relationshipProperties {
    since: Int @default(value: 2026)
  }

  type Query {
    biggestFestivals(min: Int!): [Festival!]!
      @cypher(
        statement: "MATCH (f:Festival) WHERE f.capacity >= $min RETURN f ORDER BY f.capacity DESC"
        columnName: "f"
      )
    festivalCount: Int!
      @cypher(statement: "MATCH (f:Festival) RETURN count(f) AS n")
  }

  type Mutation {
    renameGenre(key: String!, name: String!): Genre
      @cypher(
        statement: "MATCH (g:Genre) WHERE g.key = $key SET g.name = $name RETURN g"
        columnName: "g"
      )
  }
`;

/** Row-level authorization. */
export const authTypeDefs = /* GraphQL */ `
  type User @node {
    key: String! @key
    name: String
    posts: [Post!]! @relationship(type: "WROTE", direction: OUT)
  }

  type Post
    @node
    @mutation
    @authentication(operations: [CREATE, UPDATE, DELETE])
    @authorization(
      filter: [
        {
          where: { node: { published: { eq: true } } }
          requireAuthentication: false
        }
        { where: { node: { author: { key: { eq: "$jwt.sub" } } } } }
        { where: { jwt: { roles: { includes: "admin" } } } }
      ]
      validate: [
        {
          operations: [CREATE, UPDATE]
          when: [AFTER]
          where: {
            OR: [
              { node: { author: { key: { eq: "$jwt.sub" } } } }
              { jwt: { roles: { includes: "admin" } } }
            ]
          }
        }
        {
          operations: [UPDATE, DELETE]
          when: [BEFORE]
          where: {
            OR: [
              { node: { author: { key: { eq: "$jwt.sub" } } } }
              { jwt: { roles: { includes: "admin" } } }
            ]
          }
        }
      ]
    ) {
    key: ID! @key(generate: true)
    title: String! @filterable(byValue: [EQ, CONTAINS])
    published: Boolean! @default(value: false) @filterable
    secretNotes: String @authentication(operations: [READ])
    author: User! @relationship(type: "WROTE", direction: IN)
  }
`;

export const authSeed = [
  `CREATE (:User {key: 'alice', name: 'Alice'}), (:User {key: 'bob', name: 'Bob'})`,
  `MATCH (a:User {key: 'alice'}), (b:User {key: 'bob'})
   CREATE (a)-[:WROTE]->(:Post {key: 'a-pub', title: 'Alice public', published: true, secretNotes: 's1'}),
          (a)-[:WROTE]->(:Post {key: 'a-draft', title: 'Alice draft', published: false}),
          (b)-[:WROTE]->(:Post {key: 'b-draft', title: 'Bob draft', published: false})`,
];
