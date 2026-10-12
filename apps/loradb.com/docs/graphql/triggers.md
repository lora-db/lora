---
title: GraphQL Triggers, Events and Workers
sidebar_label: Triggers and workers
description: React to writes in @loradb/lora-graphql. @trigger statements and trigger functions run inside the mutation's transaction, change events report what committed, and workers deliver side effects after the commit with retries. With a complete, tested notification example.
keywords:
  [
    graphql triggers,
    database triggers,
    notifications,
    outbox pattern,
    webhooks,
    change events,
    background jobs,
  ]
---

# Triggers and workers

:::caution What fires a trigger
A trigger fires for writes made by the **generated mutations** of this
library. Raw Cypher (`db.execute()`, `transaction.execute()`) and
[`@cypher` mutations](/docs/graphql/cypher-fields) fire none, as they
skip validation and `@uniqueTogether`. A `@cypher` mutation can still
[say what it wrote](/docs/graphql/cypher-fields#saying-what-a-mutation-wrote),
so that subscriptions hear of it.
:::

Three things react to a write, at three moments:

|             | When                                                 | What it is                                                                                                                                                                                | Use it for                                                                                                    |
| ----------- | ---------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------- |
| **Trigger** | Inside the mutation's transaction, before the commit | A `@trigger` statement, or a function in the `triggers` option                                                                                                                            | Derived data that must never be out of step: a counter, an audit row, a notification record. Refusing a write |
| **Event**   | After the commit                                     | A [`WriteChange`](/docs/graphql/smart-layer#s5-write-sets-and-change-tracking) to `onWrite` and `changes()`, a change event to [subscriptions](/docs/graphql/generated-api#subscriptions) | Cache invalidation, live updates to clients                                                                   |
| **Worker**  | After the commit, retried                            | `lora.worker()` over an `@outbox` type                                                                                                                                                    | Side effects a transaction cannot hold: a push, an e-mail, a webhook                                          |

The page builds one feature with all three: a comment on a post notifies
the post's author. The responses are real: the package's test suite runs
this page.

## The callers

```json title="callers.json"
{
  "ada": { "sub": "ada" },
  "grace": { "sub": "grace" }
}
```

## The schema

```graphql title="schema.graphql"
type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
}

type Person @node {
  key: String! @key
  name: String!
}

type Post @node {
  key: String! @key
  title: String!
  commentCount: Int
  author: Person! @relationship(type: "WROTE", direction: IN)
}

type Comment
  @node
  @mutation
  @authorization(
    filter: [
      {
        operations: [UPDATE, DELETE]
        where: { node: { author: { isViewer: true } } }
      }
    ]
    validate: [
      { operations: [CREATE], where: { node: { author: { isViewer: true } } } }
    ]
  )
  @trigger(
    on: [CREATE]
    statement: """
    MATCH (this)-[:ON]->(post:Post)<-[:WROTE]-(author:Person)
    WHERE author.key <> $viewer
    CREATE (author)<-[:FOR]-(n:Notification {
      key: 'comment:' + this.key,
      kind: 'COMMENT',
      text: 'New comment on ' + post.title
    })
    RETURN n AS created
    """
  )
  @trigger(
    on: [CREATE, DELETE]
    statement: """
    MATCH (this)-[:ON]->(post:Post)
    SET post.commentCount = coalesce(post.commentCount, 0)
      + CASE $operation WHEN 'DELETE' THEN -1 ELSE 1 END
    RETURN post AS updated
    """
  )
  @trigger(
    on: [UPDATE]
    fields: ["text"]
    statement: "SET this.edited = true RETURN this AS updated"
  ) {
  key: String! @key
  text: String!
  edited: Boolean
  author: Person!
    @relationship(type: "COMMENTED", direction: IN)
    @settable(onUpdate: false)
  post: Post!
    @relationship(type: "ON", direction: OUT)
    @settable(onUpdate: false)
}

type Notification
  @node
  @outbox
  @subscription(operations: [CREATE])
  @authorization(
    filter: [{ where: { node: { recipient: { isViewer: true } } } }]
  ) {
  key: String! @key
  kind: String!
  text: String!
  recipient: Person! @relationship(type: "FOR", direction: OUT)
}
```

What is in there:

- **Three triggers on `Comment`.** The first writes a `Notification` for
  the post's author, unless the author is the one commenting. The second
  keeps `Post.commentCount`. The third marks a comment as edited, and
  only when an update sets `text`.
- **`Notification` has no `@mutation`.** Clients cannot write one: the
  only way a notification comes to exist is the trigger.
- **Its `@authorization` filter** shows each caller their own. The same
  rule guards the subscription.
- **`@outbox`** makes every notification work for a [worker](#workers).

```cypher title="seed"
CREATE (ada:Person {key: 'ada', name: 'Ada'}),
       (grace:Person {key: 'grace', name: 'Grace'}),
       (ada)-[:WROTE]->(:Post {key: 'engines', title: 'Engines'})
```

## A write and what it triggers

Grace comments on Ada's post. The comment, the counter and the
notification are one transaction: all three commit, or none does.

```graphql title="As grace"
mutation {
  createComments(
    input: [
      {
        key: "c1"
        text: "Lovely"
        author: { connect: { key: "grace" } }
        post: { connect: { key: "engines" } }
      }
    ]
  ) {
    comments {
      key
      post {
        commentCount
      }
    }
  }
}
```

```json
{
  "data": {
    "createComments": {
      "comments": [
        {
          "key": "c1",
          "post": {
            "commentCount": 1
          }
        }
      ]
    }
  }
}
```

The payload already shows `commentCount: 1`: it is read after the
triggers ran. Ada has her notification:

```graphql title="As ada"
{
  notifications {
    key
    kind
    text
  }
}
```

```json
{
  "data": {
    "notifications": [
      {
        "key": "comment:c1",
        "kind": "COMMENT",
        "text": "New comment on Engines"
      }
    ]
  }
}
```

Grace sees none, by the filter rule on `Notification`:

```graphql title="As grace"
{
  notifications {
    key
  }
}
```

```json
{
  "data": {
    "notifications": []
  }
}
```

Ada comments on her own post. The counter moves; the first trigger's
`WHERE author.key <> $viewer` writes nothing:

```graphql title="As ada"
mutation {
  createComments(
    input: [
      {
        key: "c2"
        text: "Thank you"
        author: { connect: { key: "ada" } }
        post: { connect: { key: "engines" } }
      }
    ]
  ) {
    comments {
      post {
        commentCount
      }
    }
  }
}
```

```json
{
  "data": {
    "createComments": {
      "comments": [
        {
          "post": {
            "commentCount": 2
          }
        }
      ]
    }
  }
}
```

```graphql title="As ada"
{
  notifications {
    key
  }
}
```

```json
{
  "data": {
    "notifications": [
      {
        "key": "comment:c1"
      }
    ]
  }
}
```

An update that sets `text` fires the third trigger; one that sets nothing
it names would not:

```graphql title="As grace"
mutation {
  updateComment(key: "c1", update: { text: "Lovely indeed" }) {
    comment {
      text
      edited
    }
  }
}
```

```json
{
  "data": {
    "updateComment": {
      "comment": {
        "text": "Lovely indeed",
        "edited": true
      }
    }
  }
}
```

A delete trigger runs while the node still exists, so the statement can
still walk from the comment to its post:

```graphql title="As grace"
mutation {
  deleteComment(key: "c1") {
    nodesDeleted
  }
}
```

```json
{
  "data": {
    "deleteComment": {
      "nodesDeleted": 1
    }
  }
}
```

```graphql title="As ada"
{
  posts {
    commentCount
  }
}
```

```json
{
  "data": {
    "posts": [
      {
        "commentCount": 1
      }
    ]
  }
}
```

## @trigger statements

```graphql
directive @trigger(
  on: [TriggerOperation!]!
  statement: String!
  fields: [String!]
  field: String
) repeatable on OBJECT
```

- **`on`**: `CREATE`, `UPDATE`, `DELETE`. The type needs the generated
  mutation for each (`@mutation(operations:)`); otherwise it is a startup
  error. `CONNECT` and `DISCONNECT` fire per relationship instead, with
  `field`: see [a trigger per relationship](#a-trigger-per-relationship).
- **`statement`**: Cypher, run after `this` is matched. The library puts
  `UNWIND $keys AS key MATCH (this:Comment) WHERE this.key = key` in
  front of it, so a statement runs **once per mutation**, for every node
  of the type the mutation wrote.
- **`fields`**: an `UPDATE` trigger fires only for nodes whose update
  named one of these fields.

Three parameters are bound: `$jwt` (the caller's claims, `null` without
them), `$viewer` (the caller's node's key, with a
[`@viewer` claim](/docs/graphql/authentication)) and `$operation`
(`"CREATE"`, `"UPDATE"`, `"DELETE"`, `"CONNECT"` or `"DISCONNECT"`). Any
other parameter is a startup
error. Nothing from the request is ever put into the statement's text.

Rules worth knowing:

- **Fired by what was written, not by the root field.** A comment created
  by a nested `create` inside another mutation fires the `Comment`
  triggers. An upsert fires whichever happened. A node created and
  updated in one mutation fires `CREATE` only.
- **Order.** `DELETE` triggers run before the nodes are removed.
  `CREATE` and `UPDATE` triggers run after all of the mutation's writes,
  and only once every rule has passed, type by type in schema order,
  statements before functions. A write that `@authorization`, a
  constraint or `@uniqueTogether` refuses fires none, so the error a
  denied caller gets never depends on what a trigger would have found.
  The one exception is a nested `delete:`: its `DELETE` triggers run
  once the delete's own rules passed, before the rest of the enclosing
  mutation is checked.
- **No cascades.** What a trigger writes fires no trigger. The cost of a
  mutation is readable from the schema, and a loop is impossible.
- **System writes.** A statement is your own Cypher: it is not checked
  against `@authorization`, the validation rules, `@default` or
  `@timestamp`, exactly like a `@cypher` mutation. `@uniqueTogether`
  types it may write are compared after it.
- **Checked at startup.** `lora-graphql check` and `lora.check()` plan
  every trigger statement with `explain()`: one that does not parse
  fails the check with its location.

### Telling the library what a statement wrote

A statement reports its writes in two result columns:

- `created`: a node, or a list of nodes, it created;
- `updated`: a node, or a list of nodes, it changed.

Those nodes join the mutation's write-set: `onWrite` lists them, and
subscriptions on their types fire, with no `changeFeed`. In the example
that is why Ada's open `notificationChanged` subscription hears the
notification the moment Grace's mutation commits.

The names are exact (`updated`, not `Updated`), they are columns of the
statement's last `RETURN`, and their values are nodes of the model. A
statement that writes and returns neither column, or returns something
else under them (a key, a node of a label the schema does not know),
makes the mutation's change `broad` ("anything may have changed"), which
a response cache must answer by invalidating everything. `check()`
reports a writing statement without either column as a lint.
Relationships a statement creates are not reported as connections: return
both ends as `updated` when subscribers should hear of it.

## A trigger per relationship

"Tell a person who followed them" is a trigger on the write that connects
the two, and it has to know which relationship that write made. A
`CREATE` or `UPDATE` trigger does not: `this` is the follower, and the
person followed is one relationship among all the ones the follower has.

`on: [CONNECT]` with `field:` runs the statement **once per relationship
the mutation created** through that field, with two nodes bound: `this`,
the node of the type the trigger is on, and `target`, the node at the
other end.

```graphql title="schema.graphql"
type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
}

type Person
  @node
  @mutation(operations: [UPDATE])
  @authorization(
    filter: [{ operations: [UPDATE], where: { node: { isViewer: true } } }]
  )
  @trigger(
    on: [CONNECT]
    field: "following"
    statement: """
    CREATE (target)<-[:FOR]-(n:Notification {
      key: 'follow:' + this.key + ':' + target.key,
      kind: 'FOLLOW',
      text: this.name + ' follows you'
    })
    SET target.followerCount = coalesce(target.followerCount, 0) + 1
    RETURN n AS created, target AS updated
    """
  )
  @trigger(
    on: [DISCONNECT]
    field: "following"
    statement: """
    SET target.followerCount = target.followerCount - 1
    RETURN target AS updated
    """
  ) {
  key: String! @key
  name: String!
  followerCount: Int @settable(onCreate: false, onUpdate: false)
  following: [Person!]!
    @relationship(
      type: "FOLLOWS"
      direction: OUT
      nestedOperations: [CONNECT, DISCONNECT]
    )
    @size(max: 50)
}

type Notification
  @node
  @subscription(operations: [CREATE])
  @authorization(
    filter: [{ where: { node: { recipient: { isViewer: true } } } }]
  ) {
  key: String! @key
  kind: String!
  text: String!
  recipient: Person! @relationship(type: "FOR", direction: OUT)
}
```

```cypher title="seed"
CREATE (:Person {key: 'ada', name: 'Ada'}),
       (:Person {key: 'grace', name: 'Grace'}),
       (:Person {key: 'lin', name: 'Lin'})
```

Grace follows two people in one mutation. The statement runs for each of
the two relationships, and reads no other relationship of Grace:

```graphql title="As grace"
mutation {
  updatePerson(
    key: "grace"
    update: { following: { connect: [{ key: "ada" }, { key: "lin" }] } }
  ) {
    person {
      following(sort: [{ key: ASC }]) {
        key
      }
    }
  }
}
```

```json
{
  "data": {
    "updatePerson": {
      "person": {
        "following": [
          {
            "key": "ada"
          },
          {
            "key": "lin"
          }
        ]
      }
    }
  }
}
```

```graphql title="As ada"
{
  notifications {
    key
    text
  }
  persons(where: { key: { eq: "ada" } }) {
    followerCount
  }
}
```

```json
{
  "data": {
    "notifications": [
      {
        "key": "follow:grace:ada",
        "text": "Grace follows you"
      }
    ],
    "persons": [
      {
        "followerCount": 1
      }
    ]
  }
}
```

Following someone again connects a pair that is connected already. No
relationship is created, so nothing fires, and Ada is not told twice:

```graphql title="As grace"
mutation {
  updatePerson(
    key: "grace"
    update: { following: { connect: [{ key: "ada" }] } }
  ) {
    info {
      relationshipsCreated
    }
  }
}
```

```json
{
  "data": {
    "updatePerson": {
      "info": {
        "relationshipsCreated": 0
      }
    }
  }
}
```

`on: [DISCONNECT]` runs once per relationship the mutation removed. Here
it takes the follower off the count:

```graphql title="As grace"
mutation {
  updatePerson(key: "grace", update: { following: { disconnect: ["ada"] } }) {
    info {
      relationshipsDeleted
    }
  }
}
```

```json
{
  "data": {
    "updatePerson": {
      "info": {
        "relationshipsDeleted": 1
      }
    }
  }
}
```

```graphql title="As ada"
{
  persons(sort: [{ key: ASC }]) {
    key
    followerCount
  }
}
```

```json
{
  "data": {
    "persons": [
      {
        "key": "ada",
        "followerCount": 0
      },
      {
        "key": "grace",
        "followerCount": null
      },
      {
        "key": "lin",
        "followerCount": 1
      }
    ]
  }
}
```

The rules:

- **`field`** names a relationship field of the type. `on` lists
  `CONNECT`, `DISCONNECT` or both, and none of the other three: a
  statement that runs per node has no `target`, so that is a second
  trigger.
- **Whichever side wrote it.** With `followers: [Person!]!
  @relationship(type: "FOLLOWS", direction: IN)` on the same type, a
  mutation that connects through `followers` fires the trigger on
  `following` too, and `this` is still the follower. What counts is the
  relationship: its type, and the nodes at its start and end.
- **Only relationships that are new.** `CONNECT` fires for a relationship
  the mutation created: a `connect`, a nested `create`, or either inside
  a create. A pair that was connected already fires nothing, with or
  without relationship properties in the input.
- **`DISCONNECT` needs both ends.** A relationship that goes because one
  of its nodes was deleted fires no `DISCONNECT`: there is no `this` or
  `target` left to bind. Use a `DELETE` trigger for that. The
  relationship itself is gone when the statement runs, so its properties
  cannot be read.
- **An interface or union field** fires once per member type written,
  with `target` a node of that member.
- **Reading the relationship.** On `CONNECT` it exists: `MATCH
(this)-[r:FOLLOWS]->(target)` gives its properties.
- The parameters, the `created` and `updated` columns, the order and the
  startup check are those of any trigger statement.

One mutation still connects as many people as its input lists.
[`@size(max:)`](/docs/graphql/directives#size) on the relationship field
bounds each list: above, an input that follows more than fifty people is
refused with `BAD_USER_INPUT` before anything is written.

## Trigger functions

What Cypher cannot say goes in a function:

```ts
import { GraphQLError } from "graphql";

const lora = new LoraGraphQL({
  typeDefs,
  driver: loraDriver(db),
  triggers: {
    Comment: {
      create: async ({ keys, change, transaction, context }) => {
        const { rows } = await transaction.execute(
          `UNWIND $keys AS key
           MATCH (c:Comment {key: key})-[:ON]->(p:Post)
           RETURN p.locked AS locked`,
          { keys },
        );
        if (rows.some((r) => r.locked === true)) {
          throw new GraphQLError("This post is closed for comments", {
            extensions: { code: "FORBIDDEN" },
          });
        }
      },
    },
  },
});
```

A function gets:

| Field         |                                                                       |
| ------------- | --------------------------------------------------------------------- |
| `operation`   | `"CREATE"`, `"UPDATE"` or `"DELETE"`                                  |
| `type`        | The node type                                                         |
| `keys`        | The `@key` values of the nodes written. For `DELETE` they still exist |
| `change`      | The mutation's write-set so far, `connected` and `disconnected` included |
| `transaction` | The mutation's transaction                                            |
| `context`     | The request's GraphQL context                                         |

- **Writing.** `transaction.execute(cypher, params)` runs Cypher in the
  transaction. `lora.execute({ source, context: { ...context,
transaction } })` runs a generated mutation in it: its writes join the
  write-set, and it fires no trigger itself.
- **Refusing.** Throwing rolls the whole mutation back. A `GraphQLError`
  reaches the client as thrown; any other error is reported like a
  database error (masked in production, passed to `onError`).
- **Time.** A function runs while the mutation holds the database's
  writer lock, so every other write waits for it. Past
  `triggerTimeoutMs` (default 1000) the mutation fails with `TIMEOUT`
  and rolls back. **Keep network calls out of triggers**: that is what
  workers are for.
- A function cannot commit or roll back the transaction itself.
- **Relationships.** `change.connected` and `change.disconnected` list
  the relationships the mutation wrote, each as `{ type, field, from, to
}` with `from` the node of the field's type. An update that only
  connects reaches the `update` function of the node it connects from.
  `connected` also holds a pair that was connected already and connected
  again; a `CONNECT` trigger statement is the one that tells them apart.

An unknown type, an unknown operation, or one the type has no generated
mutation for is a startup error.

## Workers

A push notification, an e-mail or a webhook cannot be part of a
transaction, and should not hold the writer lock. The pattern is an
outbox: the transaction writes a record of the work, and something
delivers it afterwards. Here the record is the `Notification` node
itself.

`@outbox` on a type makes each of its nodes work, from the moment the
write that created it commits. `lora.worker()` delivers it:

```ts
const worker = lora.worker("Notification", {
  handle: async (notification, { key, attempt, signal }) => {
    await push.send(notification.key, notification.text, { signal });
  },
});

// On shutdown: stop taking work and wait for running handlers.
await worker.stop();
```

- **At least once.** A handler may run twice for one node: a crash after
  the push went out and before the node was marked done. Make it
  idempotent; `key` is the idempotency key.
- **Retries.** A handler that throws is called again after a backoff
  (one second, doubling, at most five minutes). After `maxAttempts`
  (default 8) the node is given up as failed and `onError` is called
  with `field: "worker:Notification"` and the handler's last error. A
  call that never returned counts as an attempt too: its worker died, or
  it ran past `leaseMs`.
- **Leases.** A claim holds for `leaseMs` (default 60 seconds). Past it
  another worker may take the node, and what the first handler then
  returns is ignored. Give a handler a timeout shorter than the lease
  (the `signal` it gets aborts when the worker stops). One worker never
  runs two handlers for the same node at once.
- **Restarts.** The state lives on the node, so it is exactly as durable
  as your data. A worker looks for pending nodes when it starts and
  every `sweepMs` (default 30 seconds): work left by a crash, retries
  that are due, and claims whose worker died (`leaseMs`, default 60
  seconds).
- **Several workers.** Each node is claimed with one write before it is
  handled. LoraDB runs one write at a time, so two workers on one
  database never both get it.
- **What the handler receives.** The node's stored scalar fields by
  field name, read without read rules: a worker is server code.

| Option        | Default                     |                                                                                                          |
| ------------- | --------------------------- | -------------------------------------------------------------------------------------------------------- |
| `handle`      |                             | `(node, { key, attempt, signal }) => unknown`. Resolving marks the node done, throwing schedules a retry |
| `concurrency` | 4                           | Handlers running at once                                                                                 |
| `maxAttempts` | 8                           | Calls before the node is `FAILED`                                                                        |
| `backoffMs`   | 1 s doubling, at most 5 min | `(attempt) => number`                                                                                    |
| `sweepMs`     | 30 000                      | How often to look for work the worker was not told about                                                 |
| `leaseMs`     | 60 000                      | How long a claim holds; past it another worker may take the node                                         |

`worker.idle()` resolves when nothing is pending, which is what a test
awaits. `lora.close()` stops every worker.

What becomes work:

- a node a generated mutation created;
- a node a `@trigger` statement returned as `created`;
- a node a trigger function created with a generated mutation.

A node created by raw Cypher is not: nothing stamped it. Set
`_outbox: 'PENDING'` in the statement yourself to make it so. Nodes that
existed before the type got `@outbox` are not work either.

A handled node stays, marked done. To remove nodes of a type that is
only a queue, delete them with a generated mutation (from the handler,
or on a schedule): that runs the type's delete rules, which a worker
deleting behind the API's back would skip.

The delivery state is stored on the node in properties the library owns
(`_outbox`, `_outboxAttempts`, `_outboxAt`, `_outboxLease`,
`_outboxError`). They are not fields of the API, their writes are not
reported as changes, and `assertSchema` creates the index the worker
seeks by.

## Reaching clients

Nothing new is needed. `Notification` has `@subscription`, the trigger
reports the node as `created`, so:

```graphql
subscription {
  notificationChanged(operations: [CREATE]) {
    node {
      kind
      text
    }
  }
}
```

delivers Ada's notification to Ada's open connection, and to nobody
else: the type's `@authorization` filter is checked per subscriber. See
[subscriptions](/docs/graphql/generated-api#subscriptions) and
[serving](/docs/graphql/serving) for the WebSocket setup.

## Which one to use

- **It must be true whenever the write is true** (a counter, a
  denormalised field, an audit row, the notification record): a
  `@trigger` statement.
- **A rule that needs code** (a check against another service's cached
  data, a write shaped by a library): a trigger function. Refuse by
  throwing.
- **It leaves the database** (a push, an e-mail, a webhook, a search
  index): a worker. Have a trigger write the record, and the worker send
  it.
- **It only concerns the current process** (a response cache, a metric):
  `onWrite`.
