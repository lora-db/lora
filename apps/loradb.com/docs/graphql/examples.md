---
title: GraphQL Queries and Mutations by Example
sidebar_label: Examples
description: Forty worked requests against one @loradb/lora-graphql schema, each with its real response. Lookups, filters, relationship filters, sorting, cursor pagination, aggregates, grouping, full-text search, nested writes, upserts, atomic adjustments, bulk writes and the errors you will meet.
keywords: [graphql examples, graphql filter, cursor pagination, graphql aggregate, nested mutation, upsert, graphql cookbook]
---

# Queries and mutations by example

One schema, one small dataset, and the requests you are most likely to
write, each with the response it returns. The package's test suite runs
this page from top to bottom, so the responses are what the library
answers today.

[The generated API](/docs/graphql/generated-api) explains what each type
gets and why. This page shows it.

## The schema and the data

```graphql title="schema.graphql"
type Festival
  @node
  @mutation
  @query(aggregate: true)
  @fulltext(indexes: [{ fields: ["name", "description"] }]) {
  key: String! @key
  name: String! @filterable(byValue: [EQ, CONTAINS, STARTS_WITH]) @sortable
  description: String
  country: String! @filterable @groupBy
  capacity: Int @filterable(byValue: [GTE, LT, IS_NULL]) @sortable
  startsOn: Date @filterable(byValue: [GTE, LT]) @sortable
  tags: [String!] @filterable
  genre: Genre @relationship(type: "IN_GENRE", direction: OUT) @filterable
  stages: [Stage!]!
    @relationship(type: "HAS_STAGE", direction: OUT, onDelete: CASCADE)
  followers: [User!]!
    @relationship(type: "FOLLOWS", direction: IN, properties: "Follows")
    @filterable
}

type Genre @node {
  key: String! @key
  name: String! @filterable
}

type Stage @node @mutation {
  key: String! @key
  name: String!
}

type User @node @mutation {
  key: String! @key
  name: String! @sortable
}

type Follows @relationshipProperties {
  since: Int @filterable(byValue: [GTE]) @sortable
}
```

```cypher title="seed"
CREATE (techno:Genre {key: 'techno', name: 'Techno'}),
       (rock:Genre {key: 'rock', name: 'Rock'}),
       (jazz:Genre {key: 'jazz', name: 'Jazz'}),
       (ada:User {key: 'ada', name: 'Ada'}),
       (grace:User {key: 'grace', name: 'Grace'}),
       (linus:User {key: 'linus', name: 'Linus'}),
       (sun:Festival {
         key: 'sunland', name: 'Sunland', country: 'NL', capacity: 20000,
         description: 'Open air techno by the lake',
         startsOn: date('2027-07-02'), tags: ['outdoor', 'camping']
       }),
       (deep:Festival {
         key: 'deepfield', name: 'Deepfield', country: 'DE', capacity: 8000,
         description: 'Forest stages and late night techno',
         startsOn: date('2027-08-13'), tags: ['outdoor']
       }),
       (rockpark:Festival {
         key: 'rockpark', name: 'Rock im Park', country: 'DE',
         capacity: 70000, description: 'Three days of rock',
         startsOn: date('2027-06-04'), tags: ['outdoor', 'camping']
       }),
       (blue:Festival {
         key: 'bluenote', name: 'Blue Note Nights', country: 'NL',
         capacity: 1200, description: 'Indoor jazz sessions',
         startsOn: date('2027-11-19'), tags: ['indoor']
       }),
       (pop:Festival {
         key: 'popup', name: 'Pop-up Sessions', country: 'BE',
         description: 'Location announced on the day'
       }),
       (sun)-[:IN_GENRE]->(techno), (deep)-[:IN_GENRE]->(techno),
       (rockpark)-[:IN_GENRE]->(rock), (blue)-[:IN_GENRE]->(jazz),
       (sun)-[:HAS_STAGE]->(:Stage {key: 'sun-main', name: 'Main'}),
       (sun)-[:HAS_STAGE]->(:Stage {key: 'sun-lake', name: 'Lake'}),
       (ada)-[:FOLLOWS {since: 2021}]->(sun),
       (grace)-[:FOLLOWS {since: 2024}]->(sun),
       (ada)-[:FOLLOWS {since: 2023}]->(deep),
       (linus)-[:FOLLOWS {since: 2022}]->(rockpark)
```

Five festivals, three genres, three users. `popup` has no capacity, no
date and no genre, which makes it useful for the null cases.

:::note No access rules here
This schema is open so the examples stay about queries. A real one
guards its writes: `lora-graphql check` fails on a `@mutation` type that
nothing protects. See [authentication](/docs/graphql/authentication) and
the [authorization recipes](/docs/graphql/authorization-recipes).
:::

## Reading

### One node by key

```graphql title="Request"
{
  festival(key: "sunland") {
    name
    country
    capacity
    startsOn
    genre {
      name
    }
  }
}
```

```json
{
  "data": {
    "festival": {
      "name": "Sunland",
      "country": "NL",
      "capacity": 20000,
      "startsOn": "2027-07-02",
      "genre": { "name": "Techno" }
    }
  }
}
```

A key that does not exist is `null`, not an error:

```graphql title="Request"
{
  festival(key: "nowhere") {
    name
  }
}
```

```json
{ "data": { "festival": null } }
```

### A filtered, sorted list

Filters in one `where` are combined with AND.

```graphql title="Request"
{
  festivals(
    where: { country: { eq: "DE" }, capacity: { gte: 5000 } }
    sort: [{ capacity: DESC }]
  ) {
    name
    capacity
  }
}
```

```json
{
  "data": {
    "festivals": [
      { "name": "Rock im Park", "capacity": 70000 },
      { "name": "Deepfield", "capacity": 8000 }
    ]
  }
}
```

### OR and NOT

```graphql title="Request"
{
  festivals(
    where: {
      OR: [{ country: { eq: "BE" } }, { name: { startsWith: "Blue" } }]
      NOT: { capacity: { gte: 50000 } }
    }
    sort: [{ name: ASC }]
  ) {
    name
  }
}
```

```json
{ "data": { "festivals": [{ "name": "Blue Note Nights" }] } }
```

`popup` is in Belgium and still missing. It has no capacity, and `NOT`
over a missing value is not true, as in SQL and Cypher: the engine
cannot say that an unknown capacity is "not 50,000 or more". Add
`{ capacity: { isNull: true } }` to the `OR` when missing values should
pass.

### Text matching

```graphql title="Request"
{
  festivals(where: { name: { contains: "field" } }) {
    name
  }
}
```

```json
{ "data": { "festivals": [{ "name": "Deepfield" }] } }
```

`contains` is case sensitive. Add `CASE_INSENSITIVE` to the field's
`@filterable` for `caseInsensitive: { contains: ... }`, or use
[full-text search](#full-text-search) on larger data.

### Missing values

`eq: null` does not mean "is null": a null filter is ignored, so an unset
variable never narrows a query. Ask with `isNull`.

```graphql title="Request"
{
  festivals(where: { capacity: { isNull: true } }) {
    name
    capacity
  }
}
```

```json
{
  "data": { "festivals": [{ "name": "Pop-up Sessions", "capacity": null }] }
}
```

### Values in a list

```graphql title="Request"
{
  camping: festivals(
    where: { tags: { includes: "camping" } }
    sort: [{ name: ASC }]
  ) {
    name
  }
  benelux: festivals(
    where: { country: { in: ["NL", "BE"] } }
    sort: [{ name: ASC }]
  ) {
    name
  }
}
```

```json
{
  "data": {
    "camping": [{ "name": "Rock im Park" }, { "name": "Sunland" }],
    "benelux": [
      { "name": "Blue Note Nights" },
      { "name": "Pop-up Sessions" },
      { "name": "Sunland" }
    ]
  }
}
```

### Dates

```graphql title="Request"
{
  festivals(
    where: { startsOn: { gte: "2027-07-01", lt: "2027-09-01" } }
    sort: [{ startsOn: ASC }]
  ) {
    name
    startsOn
  }
}
```

```json
{
  "data": {
    "festivals": [
      { "name": "Sunland", "startsOn": "2027-07-02" },
      { "name": "Deepfield", "startsOn": "2027-08-13" }
    ]
  }
}
```

### By a related node

A single relationship takes the related type's filter directly.

```graphql title="Request"
{
  festivals(
    where: { genre: { name: { eq: "Techno" } } }
    sort: [{ name: ASC }]
  ) {
    name
  }
}
```

```json
{
  "data": { "festivals": [{ "name": "Deepfield" }, { "name": "Sunland" }] }
}
```

`genreExists` finds nodes with or without the relationship:

```graphql title="Request"
{
  festivals(where: { genreExists: false }) {
    name
  }
}
```

```json
{ "data": { "festivals": [{ "name": "Pop-up Sessions" }] } }
```

### By related nodes in a list

`some`, `none`, `all` and `single` quantify, and `count` compares the
number.

```graphql title="Request"
{
  followedByAda: festivals(
    where: { followers: { some: { key: { eq: "ada" } } } }
    sort: [{ name: ASC }]
  ) {
    name
  }
  noFollowers: festivals(
    where: { followers: { count: { eq: 0 } } }
    sort: [{ name: ASC }]
  ) {
    name
  }
  popular: festivals(where: { followers: { count: { gte: 2 } } }) {
    name
  }
}
```

```json
{
  "data": {
    "followedByAda": [{ "name": "Deepfield" }, { "name": "Sunland" }],
    "noFollowers": [
      { "name": "Blue Note Nights" },
      { "name": "Pop-up Sessions" }
    ],
    "popular": [{ "name": "Sunland" }]
  }
}
```

### By data on the relationship

`followersConnection` in a `where` puts a condition on the related node
and on the relationship together: festivals Ada has followed since 2022
or later.

```graphql title="Request"
{
  festivals(
    where: {
      followersConnection: {
        some: { node: { key: { eq: "ada" } }, edge: { since: { gte: 2022 } } }
      }
    }
  ) {
    name
  }
}
```

```json
{ "data": { "festivals": [{ "name": "Deepfield" }] } }
```

### Nested lists

Each level takes its own `where`, `sort` and `limit`.

```graphql title="Request"
{
  festival(key: "sunland") {
    name
    stages(sort: [{ key: ASC }]) {
      name
    }
    followers(sort: [{ name: DESC }], limit: 1) {
      name
    }
  }
}
```

```json
{
  "data": {
    "festival": {
      "name": "Sunland",
      "stages": [{ "name": "Lake" }, { "name": "Main" }],
      "followers": [{ "name": "Grace" }]
    }
  }
}
```

### Relationship properties

The connection's edges carry the relationship's own data.

```graphql title="Request"
{
  festival(key: "sunland") {
    followersConnection(sort: [{ edge: { since: ASC } }]) {
      totalCount
      edges {
        properties {
          since
        }
        node {
          name
        }
      }
    }
  }
}
```

```json
{
  "data": {
    "festival": {
      "followersConnection": {
        "totalCount": 2,
        "edges": [
          { "properties": { "since": 2021 }, "node": { "name": "Ada" } },
          { "properties": { "since": 2024 }, "node": { "name": "Grace" } }
        ]
      }
    }
  }
}
```

## Paging

### The first page

A connection returns a page, a cursor per row, and whether more follows.

```graphql title="Request"
{
  festivalsConnection(first: 2, sort: [{ name: ASC }]) {
    totalCount
    edges {
      cursor
      node {
        name
      }
    }
    pageInfo {
      hasNextPage
      endCursor
    }
  }
}
```

```json
{
  "data": {
    "festivalsConnection": {
      "totalCount": 5,
      "edges": [
        {
          "cursor": "eyJzIjoibmFtZTpBU0Msa2V5OkFTQyIsInYiOlsiQmx1ZSBOb3RlIE5pZ2h0cyIsImJsdWVub3RlIl19",
          "node": { "name": "Blue Note Nights" }
        },
        {
          "cursor": "eyJzIjoibmFtZTpBU0Msa2V5OkFTQyIsInYiOlsiRGVlcGZpZWxkIiwiZGVlcGZpZWxkIl19",
          "node": { "name": "Deepfield" }
        }
      ],
      "pageInfo": {
        "hasNextPage": true,
        "endCursor": "eyJzIjoibmFtZTpBU0Msa2V5OkFTQyIsInYiOlsiRGVlcGZpZWxkIiwiZGVlcGZpZWxkIl19"
      }
    }
  }
}
```

### The next page

Pass `endCursor` as `after`, with the same `sort`.

```graphql title="Request"
query ($after: String) {
  festivalsConnection(first: 2, after: $after, sort: [{ name: ASC }]) {
    edges {
      node {
        name
      }
    }
    pageInfo {
      hasNextPage
      hasPreviousPage
    }
  }
}
```

```json title="variables"
{ "after": "eyJzIjoibmFtZTpBU0Msa2V5OkFTQyIsInYiOlsiRGVlcGZpZWxkIiwiZGVlcGZpZWxkIl19" }
```

```json
{
  "data": {
    "festivalsConnection": {
      "edges": [
        { "node": { "name": "Pop-up Sessions" } },
        { "node": { "name": "Rock im Park" } }
      ],
      "pageInfo": { "hasNextPage": true, "hasPreviousPage": true }
    }
  }
}
```

A cursor is bound to its sort. Using it with another sort is
`INVALID_CURSOR`, and the client should start again from the first page.

### Backward

`last` and `before` page from the end.

```graphql title="Request"
{
  festivalsConnection(last: 2, sort: [{ name: ASC }]) {
    edges {
      node {
        name
      }
    }
    pageInfo {
      hasPreviousPage
    }
  }
}
```

```json
{
  "data": {
    "festivalsConnection": {
      "edges": [
        { "node": { "name": "Rock im Park" } },
        { "node": { "name": "Sunland" } }
      ],
      "pageInfo": { "hasPreviousPage": true }
    }
  }
}
```

### Only the count

Selecting nothing but `totalCount` reads no page.

```graphql title="Request"
{
  festivalsConnection(where: { country: { eq: "DE" } }) {
    totalCount
  }
}
```

```json
{ "data": { "festivalsConnection": { "totalCount": 2 } } }
```

## Aggregating

### Totals

```graphql title="Request"
{
  festivalsAggregate {
    count
    capacity {
      min
      max
      avg
      sum
    }
    name {
      shortest
      longest
    }
  }
}
```

```json
{
  "data": {
    "festivalsAggregate": {
      "count": 5,
      "capacity": { "min": 1200, "max": 70000, "avg": 24800, "sum": 99200 },
      "name": { "shortest": "Sunland", "longest": "Blue Note Nights" }
    }
  }
}
```

Aggregates skip nulls: the average is over the four festivals that have a
capacity.

### Per group

```graphql title="Request"
{
  festivalsGrouped(by: [country]) {
    by {
      country
    }
    aggregate {
      count
      capacity {
        sum
      }
    }
  }
}
```

```json
{
  "data": {
    "festivalsGrouped": [
      {
        "by": { "country": "BE" },
        "aggregate": { "count": 1, "capacity": { "sum": 0 } }
      },
      {
        "by": { "country": "DE" },
        "aggregate": { "count": 2, "capacity": { "sum": 78000 } }
      },
      {
        "by": { "country": "NL" },
        "aggregate": { "count": 2, "capacity": { "sum": 21200 } }
      }
    ]
  }
}
```

### Over a relationship

```graphql title="Request"
{
  festivals(
    where: {
      followers: { aggregate: { edge: { since: { min: { gte: 2022 } } } } }
    }
    sort: [{ name: ASC }]
  ) {
    name
  }
}
```

```json
{
  "data": {
    "festivals": [{ "name": "Deepfield" }, { "name": "Rock im Park" }]
  }
}
```

Festivals whose earliest follower joined in 2022 or later.

## Full-text search {#full-text-search}

`@fulltext` adds a search root. Terms are combined with AND, case and
accents are folded, and a trailing `*` matches a prefix.

```graphql title="Request"
{
  searchFestivals(query: "techno", where: { country: { eq: "NL" } }) {
    node {
      name
    }
  }
  prefix: searchFestivals(query: "sess*") {
    node {
      name
    }
  }
}
```

```json
{
  "data": {
    "searchFestivals": [{ "node": { "name": "Sunland" } }],
    "prefix": [
      { "node": { "name": "Blue Note Nights" } },
      { "node": { "name": "Pop-up Sessions" } }
    ]
  }
}
```

Each result also has a `score`. Results come ranked by it.

## Writing

### Create, linking existing nodes

```graphql title="Request"
mutation {
  createFestivals(
    input: [
      {
        key: "nightshift"
        name: "Nightshift"
        country: "BE"
        capacity: 3000
        genre: { connect: { key: "techno" } }
        followers: { connect: [{ key: "grace", edge: { since: 2026 } }] }
      }
    ]
  ) {
    festivals {
      key
      genre {
        name
      }
    }
    info {
      nodesCreated
      relationshipsCreated
    }
  }
}
```

```json
{
  "data": {
    "createFestivals": {
      "festivals": [{ "key": "nightshift", "genre": { "name": "Techno" } }],
      "info": { "nodesCreated": 1, "relationshipsCreated": 2 }
    }
  }
}
```

### Create with new related nodes

```graphql title="Request"
mutation {
  createFestivals(
    input: [
      {
        key: "harbour"
        name: "Harbour Jazz"
        country: "NL"
        genre: { connect: { key: "jazz" } }
        stages: {
          create: [
            { node: { key: "harbour-quay", name: "Quay" } }
            { node: { key: "harbour-ship", name: "Ship" } }
          ]
        }
      }
    ]
  ) {
    info {
      nodesCreated
      relationshipsCreated
    }
  }
}
```

```json
{
  "data": {
    "createFestivals": {
      "info": { "nodesCreated": 3, "relationshipsCreated": 3 }
    }
  }
}
```

### Update fields

`null` removes a value.

```graphql title="Request"
mutation {
  updateFestival(
    key: "nightshift"
    update: { name: "Nightshift Brussels", capacity: null }
  ) {
    festival {
      name
      capacity
    }
  }
}
```

```json
{
  "data": {
    "updateFestival": {
      "festival": { "name": "Nightshift Brussels", "capacity": null }
    }
  }
}
```

### Change a number atomically

`adjust` computes on the stored value inside the database, so two
concurrent requests cannot overwrite each other.

```graphql title="Request"
mutation {
  updateFestival(key: "sunland", adjust: { capacity: { add: 500 } }) {
    festival {
      capacity
    }
  }
}
```

```json
{ "data": { "updateFestival": { "festival": { "capacity": 20500 } } } }
```

### Change a list

```graphql title="Request"
mutation {
  updateFestival(key: "sunland", adjust: { tags: { push: ["sold-out"] } }) {
    festival {
      tags
    }
  }
}
```

```json
{
  "data": {
    "updateFestival": {
      "festival": { "tags": ["outdoor", "camping", "sold-out"] }
    }
  }
}
```

### Change links

```graphql title="Request"
mutation {
  updateFestival(
    key: "sunland"
    update: {
      genre: { connect: { key: "rock" } }
      followers: {
        connect: [{ key: "linus", edge: { since: 2027 } }]
        disconnect: ["grace"]
        update: [{ key: "ada", edge: { since: 2020 } }]
      }
    }
  ) {
    festival {
      genre {
        name
      }
      followersConnection(sort: [{ edge: { since: ASC } }]) {
        edges {
          properties {
            since
          }
          node {
            key
          }
        }
      }
    }
    info {
      relationshipsCreated
      relationshipsDeleted
    }
  }
}
```

```json
{
  "data": {
    "updateFestival": {
      "festival": {
        "genre": { "name": "Rock" },
        "followersConnection": {
          "edges": [
            { "properties": { "since": 2020 }, "node": { "key": "ada" } },
            { "properties": { "since": 2027 }, "node": { "key": "linus" } }
          ]
        }
      },
      "info": { "relationshipsCreated": 2, "relationshipsDeleted": 2 }
    }
  }
}
```

Connecting a single relationship replaces the current one, which is why
one relationship was deleted for the genre and one for Grace.

### Create or update

`upsert` creates the keys that are new and updates the others, in one
request.

```graphql title="Request"
mutation {
  upsertFestivals(
    input: [
      { key: "popup", capacity: 400 }
      { key: "afterglow", name: "Afterglow", country: "DE" }
    ]
  ) {
    festivals {
      key
      name
      capacity
    }
    info {
      nodesCreated
      nodesUpdated
    }
  }
}
```

```json
{
  "data": {
    "upsertFestivals": {
      "festivals": [
        { "key": "popup", "name": "Pop-up Sessions", "capacity": 400 },
        { "key": "afterglow", "name": "Afterglow", "capacity": null }
      ],
      "info": { "nodesCreated": 1, "nodesUpdated": 1 }
    }
  }
}
```

`name` and `country` are required only for the key that did not exist.

### Update many

A bulk update needs a `where`, and fails without writing if more nodes
match than `limit`.

```graphql title="Request"
mutation {
  updateFestivals(
    where: { country: { eq: "DE" } }
    adjust: { tags: { push: ["germany"] } }
  ) {
    info {
      nodesUpdated
    }
  }
}
```

```json
{ "data": { "updateFestivals": { "info": { "nodesUpdated": 3 } } } }
```

### Delete, with what it owns

`stages` is declared `onDelete: CASCADE`, so deleting the festival
deletes its stages. Its followers are users of their own: only the
relationships to them go.

```graphql title="Request"
mutation {
  deleteFestival(key: "sunland") {
    nodesDeleted
    relationshipsDeleted
  }
}
```

```json
{
  "data": {
    "deleteFestival": { "nodesDeleted": 3, "relationshipsDeleted": 5 }
  }
}
```

### Delete many

```graphql title="Request"
mutation {
  deleteFestivals(where: { capacity: { lt: 1000 } }, limit: 10) {
    nodesDeleted
  }
}
```

```json
{ "data": { "deleteFestivals": { "nodesDeleted": 1 } } }
```

## Errors you will meet

Errors carry a stable `extensions.code`. [Errors](/docs/graphql/errors)
lists them all.

A page larger than the maximum is refused, not silently shortened:

```graphql title="Request"
{
  festivals(limit: 500) {
    key
  }
}
```

```json
{
  "data": null,
  "errors": [
    {
      "message": "`limit` is 500; the maximum is 100",
      "extensions": { "code": "LIMIT_EXCEEDED" }
    }
  ]
}
```

Connecting to a node that does not exist fails the whole mutation:

```graphql title="Request"
mutation {
  createFestivals(
    input: [
      {
        key: "ghost"
        name: "Ghost"
        country: "NL"
        genre: { connect: { key: "polka" } }
      }
    ]
  ) {
    info {
      nodesCreated
    }
  }
}
```

```json
{
  "data": null,
  "errors": [
    {
      "message": "Festival.genre: no Genre with key \"polka\"",
      "extensions": { "code": "NOT_FOUND" }
    }
  ]
}
```

```graphql title="Request"
{
  festival(key: "ghost") {
    key
  }
}
```

```json
{ "data": { "festival": null } }
```

A key that is taken:

```graphql title="Request"
mutation {
  createFestivals(
    input: [{ key: "rockpark", name: "Again", country: "DE" }]
  ) {
    info {
      nodesCreated
    }
  }
}
```

```json
{
  "data": null,
  "errors": [
    {
      "message": "Festival.key must be unique; the value is taken",
      "extensions": { "code": "CONSTRAINT_VIOLATION" }
    }
  ]
}
```

A filter the schema did not opt into is not a runtime error. The
generated input type simply does not have it, so the document fails
validation before anything runs:

```graphql title="Request"
{
  festivals(where: { description: { contains: "lake" } }) {
    key
  }
}
```

```json
{
  "errors": [
    {
      "message": "Field \"description\" is not defined by type \"FestivalWhere\"."
    }
  ]
}
```

## Next

- [The generated API](/docs/graphql/generated-api): the full surface and
  its rules.
- [Relationships](/docs/graphql/relationships): every relationship filter
  and nested write.
- [Authorization recipes](/docs/graphql/authorization-recipes): the same
  style of worked example, per caller.
