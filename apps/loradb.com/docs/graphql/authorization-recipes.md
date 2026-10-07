---
title: GraphQL Authorization Recipes
sidebar_label: Authorization recipes
description: Complete, tested access models for @loradb/lora-graphql, each with its schema and the real response every caller gets. Private records, public reads, roles, multi-tenant isolation, teams, private fields, relationship rules and custom mutations.
keywords: [graphql authorization examples, row level security, multi-tenant, rbac, owner only, field level security]
---

# Authorization recipes

Each recipe is a complete schema for one access model, followed by the
same API called by different people. The responses are real: the
package's test suite runs this page, schema by schema and request by
request.

Read [authentication](/docs/graphql/authentication) first if you have not
set up tokens. The [authorization reference](/docs/graphql/authorization)
defines every rule used here.

## The callers

```json title="callers.json"
{
  "ada": { "sub": "ada", "roles": ["member"], "tenant": "acme" },
  "grace": { "sub": "grace", "roles": ["member"], "tenant": "acme" },
  "linus": { "sub": "linus", "roles": ["member"], "tenant": "globex" },
  "edith": { "sub": "edith", "roles": ["editor"], "tenant": "acme" },
  "root": { "sub": "root", "roles": ["admin"], "tenant": "acme" }
}
```

A block titled **As ada** runs with Ada's verified claims in the context,
and **Anonymous** with none. Every schema starts from the same claims
declaration, where `@viewer` says that `sub` is the key of the caller's
`Person` node:

```graphql
type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
  tenant: String
}
```

## Choosing a tool

| You want | Use | A caller who fails gets |
| --- | --- | --- |
| A signed-in caller, or one with a role, for a whole operation | `@authentication` | `UNAUTHENTICATED` |
| Rows the caller should not know exist | `@authorization(filter:)` | Nothing: the rows are absent |
| A write that must satisfy a condition | `@authorization(validate:)` | `FORBIDDEN`, and the write is rolled back |
| One field hidden on some rows | Field-level `validate` | `FORBIDDEN` on that field, the rest of the row intact |
| One field blanked on some rows, without an error | Field-level `mask` | A substitute value |
| Control over who links what | `validate` on the relationship field | `FORBIDDEN` |
| One role that skips every rule | `@authorizationDefaults(bypass:)` | |
| A default for every type's writes | `@authorizationDefaults(mutations:)` | `FORBIDDEN` |

The most common mistake is reaching for `validate` where `filter` is
meant. A `filter` hides; a `validate` refuses. Hiding is right for reads
(a caller should not learn that someone else's note exists), and refusing
is right for writes the caller attempted on purpose.

## Private records {#private-records}

Every note belongs to one person, and only that person can see or change
it.

```graphql title="schema.graphql"
type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
  tenant: String
}

type Person @node {
  key: String! @key
  name: String!
}

type Note
  @node
  @mutation
  @authorization(
    filter: [{ where: { node: { owner: { isViewer: true } } } }]
    validate: [
      { operations: [CREATE], where: { node: { owner: { isViewer: true } } } }
    ]
  ) {
  key: String! @key
  text: String!
  owner: Person!
    @relationship(type: "OWNS", direction: IN)
    @settable(onUpdate: false)
}
```

Three decisions are in there:

- The **filter** covers `READ`, `UPDATE` and `DELETE`: for everyone else
  the note does not exist.
- The **validate** rule covers `CREATE`, where there is no row to filter
  yet. Without it, Ada could create a note owned by Grace.
- **`@settable(onUpdate: false)`** on `owner` keeps an update from handing
  a note to someone else, which would otherwise pass the filter (it was
  Ada's when the update started).

```cypher title="seed"
CREATE (ada:Person {key: 'ada', name: 'Ada'}),
       (grace:Person {key: 'grace', name: 'Grace'}),
       (ada)-[:OWNS]->(:Note {key: 'a1', text: 'Analytical engine notes'}),
       (grace)-[:OWNS]->(:Note {key: 'g1', text: 'Compiler ideas'})
```

Ada lists notes and gets hers:

```graphql title="As ada"
{
  notes {
    key
    text
  }
}
```

```json
{
  "data": { "notes": [{ "key": "a1", "text": "Analytical engine notes" }] }
}
```

Asking for Grace's note by key answers as if it did not exist. So does
trying to change it:

```graphql title="As ada"
{
  note(key: "g1") {
    text
  }
}
```

```json
{ "data": { "note": null } }
```

```graphql title="As ada"
mutation {
  updateNote(key: "g1", update: { text: "hacked" }) {
    note {
      key
    }
  }
  deleteNote(key: "g1") {
    nodesDeleted
  }
}
```

```json
{
  "data": {
    "updateNote": { "note": null },
    "deleteNote": { "nodesDeleted": 0 }
  }
}
```

Creating a note for herself works. Creating one in Grace's name does not:

```graphql title="As ada"
mutation {
  createNotes(
    input: [
      {
        key: "a2"
        text: "Bernoulli numbers"
        owner: { connect: { key: "ada" } }
      }
    ]
  ) {
    notes {
      key
    }
  }
}
```

```json
{ "data": { "createNotes": { "notes": [{ "key": "a2" }] } } }
```

```graphql title="As ada"
mutation {
  createNotes(
    input: [
      { key: "x1", text: "planted", owner: { connect: { key: "grace" } } }
    ]
  ) {
    notes {
      key
    }
  }
}
```

```json
{
  "data": null,
  "errors": [
    {
      "message": "not allowed to create this Note",
      "extensions": { "code": "FORBIDDEN" }
    }
  ]
}
```

An anonymous caller sees nothing. Rules deny without a token by default,
and for a filter "deny" means no rows, not an error. Add
`@authentication` to the type if a missing token should be refused with
`UNAUTHENTICATED` instead:

```graphql title="Anonymous"
{
  notes {
    key
  }
}
```

```json
{ "data": { "notes": [] } }
```

## Public read, owner write {#public-read-owner-write}

A blog: anyone reads published posts, authors also see their own drafts,
and only the author changes a post.

```graphql title="schema.graphql"
extend schema @authorizationDefaults(requireAuthentication: false)

type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
  tenant: String
}

type Person @node {
  key: String! @key
  name: String!
}

type Post
  @node
  @mutation
  @authentication(operations: [CREATE, UPDATE, DELETE])
  @authorization(
    filter: [
      { operations: [READ], where: { node: { published: { eq: true } } } }
      { where: { node: { author: { isViewer: true } } } }
    ]
    validate: [
      {
        operations: [CREATE]
        where: { node: { author: { isViewer: true } } }
      }
    ]
  ) {
  slug: String! @key
  title: String!
  published: Boolean! @default(value: false)
  author: Person!
    @relationship(type: "WROTE", direction: IN)
    @settable(onUpdate: false)
}
```

- `requireAuthentication: false` lets the first filter rule, which reads
  no claim, decide for anonymous callers too. Without it they would see
  nothing.
- Any passing filter rule grants. The first is for `READ` only, so
  being published makes a post readable but not editable. The second has
  the default operations (`READ`, `UPDATE`, `DELETE`), so the author can
  do all three.
- `@authentication` on the writes gives anonymous callers a clear
  `UNAUTHENTICATED` instead of an empty result.

```cypher title="seed"
CREATE (ada:Person {key: 'ada', name: 'Ada'}),
       (grace:Person {key: 'grace', name: 'Grace'}),
       (ada)-[:WROTE]->(:Post {
         slug: 'engines', title: 'On engines', published: true
       }),
       (ada)-[:WROTE]->(:Post {
         slug: 'draft', title: 'Unfinished', published: false
       }),
       (grace)-[:WROTE]->(:Post {
         slug: 'cobol', title: 'COBOL', published: true
       })
```

```graphql title="Anonymous"
{
  posts(sort: [{ slug: ASC }]) {
    slug
    author {
      name
    }
  }
}
```

```json
{
  "data": {
    "posts": [
      { "slug": "cobol", "author": { "name": "Grace" } },
      { "slug": "engines", "author": { "name": "Ada" } }
    ]
  }
}
```

Ada also sees her draft:

```graphql title="As ada"
{
  posts(sort: [{ slug: ASC }]) {
    slug
    published
  }
}
```

```json
{
  "data": {
    "posts": [
      { "slug": "cobol", "published": true },
      { "slug": "draft", "published": false },
      { "slug": "engines", "published": true }
    ]
  }
}
```

Grace can read Ada's published post but not change it. The update finds
no post she may update, so it changes nothing:

```graphql title="As grace"
mutation {
  updatePost(slug: "engines", update: { title: "Defaced" }) {
    post {
      slug
    }
    info {
      nodesUpdated
    }
  }
}
```

```json
{
  "data": { "updatePost": { "post": null, "info": { "nodesUpdated": 0 } } }
}
```

Ada publishes her draft:

```graphql title="As ada"
mutation {
  updatePost(slug: "draft", update: { published: true }) {
    post {
      slug
      published
    }
  }
}
```

```json
{
  "data": {
    "updatePost": { "post": { "slug": "draft", "published": true } }
  }
}
```

```graphql title="Anonymous"
mutation {
  deletePost(slug: "cobol") {
    nodesDeleted
  }
}
```

```json
{
  "data": null,
  "errors": [
    {
      "message": "delete on Post needs an authenticated request",
      "extensions": { "code": "UNAUTHENTICATED" }
    }
  ]
}
```

## Roles {#roles}

A catalogue everyone reads, editors maintain, and admins administer
without any rule in their way.

```graphql title="schema.graphql"
extend schema
  @authorizationDefaults(
    requireAuthentication: false
    mutations: { jwt: { roles: { includes: "editor" } } }
    bypass: { jwt: { roles: { includes: "admin" } } }
  )

type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
  tenant: String
}

type Person @node {
  key: String! @key
}

type Product @node @mutation {
  sku: String! @key
  name: String!
  price: Int!
}

type PriceChange
  @node
  @mutation(operations: [CREATE])
  @authorization(filter: [{ where: { node: { by: { isViewer: true } } } }]) {
  key: ID! @key(generate: true)
  reason: String!
  by: Person! @relationship(type: "MADE", direction: IN)
}
```

- `mutations` is the write rule of every `@mutation` type that declares
  none of its own. `Product` has no rules, so its creates, updates and
  deletes need the `editor` role. A new type added next month is covered
  the day it is added.
- `bypass` skips every filter and validate rule for admins. It is decided
  from the token before a statement is built, so an admin's query carries
  no rule predicate at all.
- `PriceChange` has a filter (people see their own), which covers reads.
  Its `CREATE` has no rule of its own, so the `mutations` default applies
  to it too.

```cypher title="seed"
CREATE (:Product {sku: 'p1', name: 'Punch card', price: 2}),
       (edith:Person {key: 'edith'}), (:Person {key: 'root'}),
       (edith)-[:MADE]->(:PriceChange {key: 'c1', reason: 'Paper shortage'})
```

A member reads but cannot write:

```graphql title="As ada"
{
  products {
    sku
    price
  }
}
```

```json
{ "data": { "products": [{ "sku": "p1", "price": 2 }] } }
```

```graphql title="As ada"
mutation {
  updateProduct(sku: "p1", update: { price: 0 }) {
    product {
      price
    }
  }
}
```

```json
{
  "data": null,
  "errors": [
    {
      "message": "not allowed to update this Product",
      "extensions": { "code": "FORBIDDEN" }
    }
  ]
}
```

An editor can:

```graphql title="As edith"
mutation {
  updateProduct(sku: "p1", update: { price: 3 }) {
    product {
      price
    }
  }
}
```

```json
{ "data": { "updateProduct": { "product": { "price": 3 } } } }
```

The admin has no `PriceChange` of their own, and sees Edith's anyway:

```graphql title="As root"
{
  priceChanges {
    key
    reason
  }
}
```

```json
{
  "data": { "priceChanges": [{ "key": "c1", "reason": "Paper shortage" }] }
}
```

```graphql title="As ada"
{
  priceChanges {
    key
  }
}
```

```json
{ "data": { "priceChanges": [] } }
```

To keep a type's rules in force for admins too, add
`@authorization(bypass: false)` to it. `lora-graphql check` lists every
type the bypass reaches, so one cannot join it unnoticed.

## Multi-tenant isolation {#multi-tenant}

One database, many customers, and no customer sees another's rows. The
tenant comes from a verified claim, never from an argument.

```graphql title="schema.graphql"
type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
  tenant: String
}

type Person @node {
  key: String! @key
}

type Document
  @node
  @mutation
  @authorization(
    filter: [{ where: { node: { tenant: { eq: "$jwt.tenant" } } } }]
    validate: [
      {
        operations: [CREATE]
        where: { node: { tenant: { eq: "$jwt.tenant" } } }
      }
    ]
  ) {
  key: String! @key
  tenant: String! @settable(onUpdate: false) @filterable
  title: String! @sortable
}
```

- `"$jwt.tenant"` is bound as a parameter, so all callers share one
  compiled statement and the engine can seek on `tenant`.
- The `CREATE` rule stops a caller from writing into another tenant, and
  `@settable(onUpdate: false)` stops a document from being moved.
- A caller whose token has no `tenant` claim matches nothing: a missing
  claim makes a rule unknown, which denies.

```cypher title="seed"
CREATE (:Document {key: 'd1', tenant: 'acme', title: 'Acme roadmap'}),
       (:Document {key: 'd2', tenant: 'acme', title: 'Acme payroll'}),
       (:Document {key: 'd3', tenant: 'globex', title: 'Globex merger'})
```

```graphql title="As ada"
{
  documents(sort: [{ title: ASC }]) {
    key
    title
  }
}
```

```json
{
  "data": {
    "documents": [
      { "key": "d2", "title": "Acme payroll" },
      { "key": "d1", "title": "Acme roadmap" }
    ]
  }
}
```

```graphql title="As linus"
{
  documents {
    key
    title
  }
  document(key: "d1") {
    title
  }
}
```

```json
{
  "data": {
    "documents": [{ "key": "d3", "title": "Globex merger" }],
    "document": null
  }
}
```

Counts and aggregates obey the same filter, so a total cannot leak the
size of another tenant.

Linus cannot write into Acme, even by naming it:

```graphql title="As linus"
mutation {
  createDocuments(input: [{ key: "d9", tenant: "acme", title: "Planted" }]) {
    documents {
      key
    }
  }
}
```

```json
{
  "data": null,
  "errors": [
    {
      "message": "not allowed to create this Document",
      "extensions": { "code": "FORBIDDEN" }
    }
  ]
}
```

```graphql title="As linus"
mutation {
  createDocuments(
    input: [{ key: "d4", tenant: "globex", title: "Globex plan" }]
  ) {
    documents {
      key
      tenant
    }
  }
}
```

```json
{
  "data": {
    "createDocuments": {
      "documents": [{ "key": "d4", "tenant": "globex" }]
    }
  }
}
```

Clients should not have to send their own tenant. Fill it from the token
with `@populatedBy`, which takes the field out of the input altogether:

```graphql
tenant: String!
  @populatedBy(callback: "tenant", operations: [CREATE])
  @filterable
```

```ts
new LoraGraphQL({
  typeDefs,
  driver: loraDriver(db),
  callbacks: { tenant: ({ context }) => context.jwt.tenant },
});
```

Every tenant-owned type needs the same two rules. Write the claim test
once as a [named rule](/docs/graphql/authorization#named-rules) if it is
more than one line, and let `lora-graphql access` confirm that no type
was missed.

## Teams and membership {#teams-and-membership}

Access that follows relationships: a project is visible to its owner and
its members, the owner decides who is a member, and tasks inherit the
project's visibility.

```graphql title="schema.graphql"
type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
  tenant: String
}

type Person @node {
  key: String! @key
}

type Project
  @node
  @mutation(operations: [UPDATE])
  @authorizationRule(
    name: "member"
    where: {
      OR: [
        { node: { owner: { isViewer: true } } }
        { node: { members: { some: { isViewer: true } } } }
      ]
    }
  )
  @authorization(filter: [{ where: { rule: "member" } }]) {
  key: String! @key
  name: String!
    @authorization(
      validate: [
        {
          operations: [UPDATE]
          where: { node: { owner: { isViewer: true } } }
        }
      ]
    )
  owner: Person!
    @relationship(type: "OWNS", direction: IN, nestedOperations: [])
  members: [Person!]!
    @relationship(
      type: "MEMBER_OF"
      direction: IN
      nestedOperations: [CONNECT, DISCONNECT]
    )
    @authorization(
      validate: [
        {
          operations: [CONNECT]
          where: { source: { owner: { isViewer: true } } }
        }
        {
          operations: [DISCONNECT]
          where: {
            OR: [
              { source: { owner: { isViewer: true } } }
              { target: { isViewer: true } }
            ]
          }
        }
      ]
    )
}

type Task
  @node
  @authorization(
    filter: [{ where: { node: { project: { rule: "member" } } } }]
  ) {
  key: String! @key
  title: String!
  project: Project! @relationship(type: "PART_OF", direction: OUT)
}
```

- `@authorizationRule` names "is a member" once. `Project` uses it as
  `{ rule: "member" }`, and `Task` reaches it through its relationship
  with `project: { rule: "member" }`. Change the definition and both
  follow.
- The filter lets members read and update the project. It has to admit
  them for updates, because leaving a project is an update of it: a
  member the filter hid the project from could not even reach the rule
  that lets them leave.
- What members may not do is narrowed field by field. The rule on `name`
  lets only the owner rename the project.
- The rules on `members` are about the link, with both ends in view:
  `source` is the project and `target` the person. Only the owner adds
  people. The owner removes anyone, and a member may remove themselves.
- `nestedOperations: []` on `owner` makes ownership unchangeable through
  the API.

```cypher title="seed"
CREATE (ada:Person {key: 'ada'}),
       (grace:Person {key: 'grace'}),
       (linus:Person {key: 'linus'}),
       (ada)-[:OWNS]->(p:Project {key: 'engine', name: 'Analytical Engine'}),
       (:Task {key: 't1', title: 'Order brass'})-[:PART_OF]->(p)
```

Grace is not a member yet, so the project and its tasks are invisible to
her:

```graphql title="As grace"
{
  projects {
    key
  }
  tasks {
    key
  }
}
```

```json
{ "data": { "projects": [], "tasks": [] } }
```

Ada, the owner, adds her:

```graphql title="As ada"
mutation {
  updateProject(
    key: "engine"
    update: { members: { connect: [{ key: "grace" }] } }
  ) {
    project {
      members {
        key
      }
    }
  }
}
```

```json
{
  "data": {
    "updateProject": { "project": { "members": [{ "key": "grace" }] } }
  }
}
```

```graphql title="As grace"
{
  projects {
    name
  }
  tasks {
    title
  }
}
```

```json
{
  "data": {
    "projects": [{ "name": "Analytical Engine" }],
    "tasks": [{ "title": "Order brass" }]
  }
}
```

Grace is a member, not the owner. She cannot add Linus, and she cannot
rename the project:

```graphql title="As grace"
mutation {
  updateProject(
    key: "engine"
    update: { members: { connect: [{ key: "linus" }] } }
  ) {
    info {
      relationshipsCreated
    }
  }
}
```

```json
{
  "data": null,
  "errors": [
    {
      "message": "not allowed to connect Project.members",
      "extensions": { "code": "FORBIDDEN" }
    }
  ]
}
```

```graphql title="As grace"
mutation {
  updateProject(key: "engine", update: { name: "Grace's Engine" }) {
    project {
      name
    }
  }
}
```

```json
{
  "data": null,
  "errors": [
    {
      "message": "not allowed to update this Project",
      "extensions": { "code": "FORBIDDEN" }
    }
  ]
}
```

She can leave:

```graphql title="As grace"
mutation {
  updateProject(
    key: "engine"
    update: { members: { disconnect: ["grace"] } }
  ) {
    info {
      relationshipsDeleted
    }
  }
}
```

```json
{ "data": { "updateProject": { "info": { "relationshipsDeleted": 1 } } } }
```

And with that the project is invisible to her again:

```graphql title="As grace"
{
  projects {
    key
  }
}
```

```json
{ "data": { "projects": [] } }
```

## Private fields {#private-fields}

Everyone sees a person's name. Only the person themselves sees their
email and their phone number. The two fields show the two ways to hide a
value.

```graphql title="schema.graphql"
type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
  tenant: String
}

type Person @node {
  key: String! @key
  name: String!
  email: String
    @authorization(
      validate: [{ operations: [READ], where: { node: { isViewer: true } } }]
    )
  phone: String
    @authorization(mask: [{ unless: { node: { isViewer: true } } }])
}
```

```cypher title="seed"
CREATE (:Person {
         key: 'ada', name: 'Ada',
         email: 'ada@example.com', phone: '555-0100'
       }),
       (:Person {
         key: 'grace', name: 'Grace',
         email: 'grace@example.com', phone: '555-0199'
       })
```

A **mask** substitutes a value, `null` by default, and raises no error.
The response is clean and the client cannot tell a hidden phone number
from a missing one:

```graphql title="As ada"
{
  persons(sort: [{ key: ASC }]) {
    name
    phone
  }
}
```

```json
{
  "data": {
    "persons": [
      { "name": "Ada", "phone": "555-0100" },
      { "name": "Grace", "phone": null }
    ]
  }
}
```

A **validate** rule fails the field on rows that do not pass, and says
so. The rest of the row still arrives:

```graphql title="As ada"
{
  persons(sort: [{ key: ASC }]) {
    name
    email
  }
}
```

```json
{
  "data": {
    "persons": [
      { "name": "Ada", "email": "ada@example.com" },
      { "name": "Grace", "email": null }
    ]
  },
  "errors": [
    {
      "message": "not allowed to read Person.email",
      "extensions": { "code": "FORBIDDEN" }
    }
  ]
}
```

Choose the mask when clients list many rows and should simply not see the
value. Choose the rule when reading the field at all is a mistake the
client should hear about.

Neither can be probed through a filter. A `where` on a masked field
compares the value the reader sees, and a `where` on a ruled field only
matches rows the reader passes.

## Roles on a relationship {#relationship-properties}

A membership carries a role, and only admins hand out roles. Anyone
signed in may join as a plain member.

```graphql title="schema.graphql"
type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
  tenant: String
}

type Person @node {
  key: String! @key
}

type Org
  @node
  @mutation(operations: [UPDATE])
  @authentication(operations: [UPDATE]) {
  key: String! @key
  members: [Person!]!
    @relationship(
      type: "BELONGS_TO"
      direction: IN
      properties: "Membership"
      nestedOperations: [CONNECT]
    )
    @authorization(
      validate: [
        {
          operations: [CONNECT, UPDATE_EDGE]
          where: {
            OR: [
              { target: { isViewer: true } }
              { jwt: { roles: { includes: "admin" } } }
            ]
          }
        }
      ]
    )
}

type Membership @relationshipProperties {
  role: String!
    @default(value: "member")
    @authorization(
      validate: [
        {
          operations: [CREATE, UPDATE]
          where: { jwt: { roles: { includes: "admin" } } }
        }
      ]
    )
}
```

- The rule on `members` lets a caller connect only themselves, and lets
  admins connect anyone.
- The rule on `role` guards the property. It is checked only when the
  input sets the property, so joining without a role is allowed and gets
  the `@default`.

```cypher title="seed"
CREATE (:Org {key: 'acme'}), (:Person {key: 'ada'}), (:Person {key: 'grace'})
```

Ada joins, and gets the default role:

```graphql title="As ada"
mutation {
  updateOrg(key: "acme", update: { members: { connect: [{ key: "ada" }] } }) {
    org {
      membersConnection {
        edges {
          properties {
            role
          }
          node {
            key
          }
        }
      }
    }
  }
}
```

```json
{
  "data": {
    "updateOrg": {
      "org": {
        "membersConnection": {
          "edges": [
            { "properties": { "role": "member" }, "node": { "key": "ada" } }
          ]
        }
      }
    }
  }
}
```

Grace tries to join as an owner:

```graphql title="As grace"
mutation {
  updateOrg(
    key: "acme"
    update: {
      members: { connect: [{ key: "grace", edge: { role: "owner" } }] }
    }
  ) {
    info {
      relationshipsCreated
    }
  }
}
```

```json
{
  "data": null,
  "errors": [
    {
      "message": "not allowed to create Membership.role",
      "extensions": { "code": "FORBIDDEN" }
    }
  ]
}
```

An admin makes her one:

```graphql title="As root"
mutation {
  updateOrg(
    key: "acme"
    update: {
      members: { connect: [{ key: "grace", edge: { role: "owner" } }] }
    }
  ) {
    org {
      membersConnection(sort: [{ key: ASC }]) {
        edges {
          properties {
            role
          }
          node {
            key
          }
        }
      }
    }
  }
}
```

```json
{
  "data": {
    "updateOrg": {
      "org": {
        "membersConnection": {
          "edges": [
            {
              "properties": { "role": "member" },
              "node": { "key": "ada" }
            },
            {
              "properties": { "role": "owner" },
              "node": { "key": "grace" }
            }
          ]
        }
      }
    }
  }
}
```

## Keys in the caller's namespace {#owner-scoped-keys}

When keys are chosen by clients, let each caller create only under their
own prefix. No rule to write: it is an argument of `@key`.

```graphql title="schema.graphql"
type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
  tenant: String
}

type Person @node {
  key: String! @key
}

type Trip
  @node
  @mutation(operations: [CREATE])
  @authentication(operations: [CREATE])
  @authorization(
    filter: [{ where: { node: { key: { startsWith: "${viewer.key}:" } } } }]
  ) {
  key: String! @key(scope: VIEWER)
  name: String!
}
```

```cypher title="seed"
CREATE (:Person {key: 'ada'}), (:Person {key: 'grace'})
```

```graphql title="As ada"
mutation {
  createTrips(input: [{ key: "ada:paris", name: "Paris" }]) {
    trips {
      key
    }
  }
}
```

```json
{ "data": { "createTrips": { "trips": [{ "key": "ada:paris" }] } } }
```

```graphql title="As ada"
mutation {
  createTrips(input: [{ key: "grace:paris", name: "Not mine" }]) {
    trips {
      key
    }
  }
}
```

```json
{
  "data": null,
  "errors": [
    {
      "message": "not allowed to create this Trip",
      "extensions": { "code": "FORBIDDEN" }
    }
  ]
}
```

The check runs before any statement, so the answer for `grace:paris` is
the same whether or not that trip exists. The filter uses the same
prefix to make each caller's trips private:

```graphql title="As ada"
{
  trips {
    key
    name
  }
}
```

```json
{ "data": { "trips": [{ "key": "ada:paris", "name": "Paris" }] } }
```

```graphql title="As grace"
{
  trips {
    key
  }
}
```

```json
{ "data": { "trips": [] } }
```

## A guarded custom mutation {#custom-mutation}

Generated mutations cover keyed writes. A state change with its own
precondition is clearer as one `@cypher` mutation with a guard: here,
only verified people may publish, and only their own listings.

```graphql title="schema.graphql"
type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
  tenant: String
}

type Person @node {
  key: String! @key
  verified: Boolean!
}

type Listing @node {
  key: String! @key
  status: String!
  seller: Person! @relationship(type: "SELLS", direction: IN)
}

type Mutation {
  publishListing(key: String!): Listing
    @authentication
    @authorization(
      validate: [{ where: { viewer: { verified: { eq: true } } } }]
    )
    @cypher(
      statement: """
      MATCH (:Person {key: $viewer})-[:SELLS]->(l:Listing {key: $key})
      SET l.status = 'PUBLISHED'
      RETURN l
      """
    )
}
```

- `viewer: { verified: { eq: true } }` tests the caller's own node, with
  one seek, before the statement runs.
- The statement itself enforces ownership by matching from `$viewer`.
  Rules on `Listing` would not apply inside it: a `@cypher` statement
  runs with full access, so the statement must say what it means.

```cypher title="seed"
CREATE (ada:Person {key: 'ada', verified: true}),
       (grace:Person {key: 'grace', verified: false}),
       (ada)-[:SELLS]->(:Listing {key: 'l1', status: 'DRAFT'}),
       (grace)-[:SELLS]->(:Listing {key: 'l2', status: 'DRAFT'})
```

```graphql title="As grace"
mutation {
  publishListing(key: "l2") {
    status
  }
}
```

```json
{
  "data": { "publishListing": null },
  "errors": [
    {
      "message": "not allowed to run Mutation.publishListing",
      "extensions": { "code": "FORBIDDEN" }
    }
  ]
}
```

```graphql title="As ada"
mutation {
  publishListing(key: "l1") {
    key
    status
  }
}
```

```json
{ "data": { "publishListing": { "key": "l1", "status": "PUBLISHED" } } }
```

Ada is verified but does not own `l2`, so the statement matches nothing
and the field is `null`:

```graphql title="As ada"
mutation {
  publishListing(key: "l2") {
    status
  }
}
```

```json
{ "data": { "publishListing": null } }
```

## Prove it

Rules are easy to get subtly wrong, so check them the way you check
code.

**Read the matrix.** `lora-graphql access schema.graphql` prints who may
do what. For the [private records](#private-records) schema:

```text
Note    READ    anonymous      denied           filter[0]
Note    READ    authenticated  filtered         filter[0]
Note    READ    viewer         allowed          filter[0]
Note    CREATE  anonymous      unauthenticated  validate[0]
Note    CREATE  authenticated  validated        validate[0]
Note    CREATE  viewer         allowed          validate[0]
Note    UPDATE  anonymous      denied           filter[0]
Note    UPDATE  authenticated  filtered         filter[0]
Note    UPDATE  viewer         allowed          filter[0]
Note    DELETE  anonymous      denied           filter[0]
Note    DELETE  authenticated  filtered         filter[0]
Note    DELETE  viewer         allowed          filter[0]
Person  READ    anonymous      allowed          no rule
Person  READ    authenticated  allowed          no rule
Person  READ    viewer         allowed          no rule
```

Each line is a type, an operation, a kind of caller, the verdict and the
rule that decides it. `authenticated` is any signed-in caller, and
`viewer` is the caller on rows that are theirs. Read it for surprises:
the last three lines say every `Person` is readable by anyone, anonymous
callers included, which is true of this schema and may not be what you
want in yours.

Commit that output and diff it in CI: a pull request that changes who can
do what then shows it in review.

**Gate the schema.** `lora-graphql check schema.graphql` fails when a
`@mutation` type has a write that no rule or `@authentication` guards,
and lints the patterns that usually mean a mistake, such as an update
rule that tests a relationship the same update can re-point.

**Probe it as each caller.** `expectAccess` states the model as a table
and runs every line against a real database, in transactions that are
rolled back:

```ts title="notes.access.test.ts"
import { test } from "vitest";
import {
  createTestLoraGraphQL,
  expectAccess,
} from "@loradb/lora-graphql/testing";

test("notes are private to their owner", async () => {
  const t = await createTestLoraGraphQL({ typeDefs, seed });

  await expectAccess(t, {
    as: { sub: "ada" },
    allowed: ["read Note a1", "update Note a1", "delete Note a1"],
    denied: ["read Note g1", "update Note g1", "delete Note g1"],
  });

  // An anonymous caller is spelled out, so it is never one by accident.
  await expectAccess(t, { as: undefined, denied: ["read Note a1"] });
});
```

A wrong expectation fails with every mismatch listed:

```text
access as {"sub":"ada"} differs in 1 entry:
  expected allowed, was denied: read Note g1 (not visible)
```

See [testing](/docs/graphql/testing#asserting-access).

## Next

- [Authorization reference](/docs/graphql/authorization): every rule,
  operator and default.
- [Authentication](/docs/graphql/authentication): verifying tokens and
  requiring callers.
- [Relationships](/docs/graphql/relationships): the fields relationship
  rules sit on.
