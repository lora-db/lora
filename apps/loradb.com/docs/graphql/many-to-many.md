---
title: Many-to-many relationships
sidebar_label: Many-to-many
description: How a relational many-to-many, with its join table, foreign keys and join columns, becomes one @relationship in @loradb/lora-graphql, and how to read, filter, write and page it from both sides.
keywords: [many-to-many, join table, junction table, graphql, relationship properties, sql to graph]
---

import JoinTableDiagram from "@site/src/components/JoinTableDiagram";

# Many-to-many relationships

In a relational database a many-to-many needs a third table. Students take
many courses, a course has many students, so you add `enrollments` with two
foreign keys, index both, and join through it in every query.

In a graph the join table is the relationship. There is no third table, no
foreign key and no join to write. This page takes the relational model
piece by piece and shows what each piece becomes in
`@loradb/lora-graphql`, using one example throughout. The operations run
in the order shown, each on the data the previous ones left, and the
package's test suite runs this page to keep the printed results true.

<JoinTableDiagram />

## The relational model

```sql
CREATE TABLE students (
  id   TEXT PRIMARY KEY,
  name TEXT NOT NULL
);

CREATE TABLE courses (
  code    TEXT PRIMARY KEY,
  title   TEXT NOT NULL,
  credits INT
);

CREATE TABLE enrollments (
  student_id  TEXT NOT NULL REFERENCES students (id) ON DELETE CASCADE,
  course_code TEXT NOT NULL REFERENCES courses (code) ON DELETE CASCADE,
  enrolled_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  grade       REAL,
  role        TEXT NOT NULL DEFAULT 'student',
  PRIMARY KEY (student_id, course_code)
);

CREATE INDEX enrollments_course ON enrollments (course_code);
```

The join table carries four separate ideas: the link itself, the rule that
a pair is linked at most once (the composite primary key), data about the
link (`grade`, `role`, `enrolled_at`), and what happens to links when a row
on either side goes away.

## The same model as a graph

```graphql title="schema.graphql"
type Student @node @mutation @query(aggregate: true) {
  id: ID! @key(generate: true)
  name: String! @filterable(byValue: [EQ, CONTAINS]) @sortable
  courses: [Course!]!
    @relationship(
      type: "ENROLLED_IN"
      direction: OUT
      properties: "Enrollment"
    )
    @filterable
}

type Course @node @mutation @query(aggregate: true) {
  code: String! @key
  title: String! @filterable(byValue: [EQ, CONTAINS]) @sortable
  credits: Int @filterable(byValue: [GTE, LTE]) @sortable
  students: [Student!]!
    @relationship(
      type: "ENROLLED_IN"
      direction: IN
      properties: "Enrollment"
    )
    @filterable
}

type Enrollment @relationshipProperties {
  enrolledAt: DateTime @timestamp(operations: [CREATE])
  grade: Float @filterable(byValue: [GTE, LT]) @sortable
  role: String @default(value: "student")
}
```

| Relational | lora-graphql |
|---|---|
| Table | A type with `@node` |
| Primary key | The `@key` field |
| Join table | A relationship type, here `ENROLLED_IN` |
| The two foreign keys | The two `@relationship` fields, one on each type |
| Composite primary key on the pair | Built in: the API keeps one relationship per pair |
| Extra columns on the join table | A `@relationshipProperties` type |
| `DEFAULT` on a join column | `@default` or `@timestamp` on the property |
| Index on each foreign key | Nothing. A node holds its relationships |
| `ON DELETE CASCADE` on the foreign keys | Built in: deleting a node removes its relationships |

Three things to notice in the schema.

**One relationship, declared from both ends.** `Student.courses` and
`Course.students` name the same type, `ENROLLED_IN`, with opposite
directions. That is one set of relationships in the database seen from two
sides, not two sets to keep in sync. Connect a course to a student and the
student appears under the course.

**Direction is a storage detail, not an access restriction.** A
relationship is stored with a direction (`Student` to `Course` here), and
`OUT` or `IN` says which end the field sits on. Reading is equally cheap
from either end, so pick the direction that reads naturally and keep it
consistent.

**The join columns move onto the relationship.** `Enrollment` is not a
node. It has no key and no root field. It describes the properties each
`ENROLLED_IN` relationship carries, and both fields point at it with
`properties: "Enrollment"`.

`assertSchema({ create: true })` creates the key constraints and the
indexes for the `@filterable` and `@sortable` node fields. It creates
nothing for the relationship, because there is nothing to index: where SQL
looks a student id up in an index on `enrollments`, the graph follows the
relationships already attached to the student node.

## Write the links

Start with a few courses, the plain `INSERT INTO courses`:

```graphql
mutation {
  createCourses(
    input: [
      { code: "CS101", title: "Algorithms", credits: 6 }
      { code: "MA201", title: "Linear Algebra", credits: 5 }
      { code: "PH110", title: "Mechanics", credits: 4 }
    ]
  ) {
    info {
      nodesCreated
    }
  }
}
```

```json
{ "createCourses": { "info": { "nodesCreated": 3 } } }
```

### Link rows that exist

`connect` links to existing nodes by their key. It is the `INSERT INTO
enrollments`. The `edge` object sets the relationship properties.

```graphql
mutation {
  createStudents(
    input: [
      {
        id: "s1"
        name: "Ada"
        courses: {
          connect: [
            { code: "CS101", edge: { grade: 9.1 } }
            { code: "MA201" }
          ]
        }
      }
    ]
  ) {
    students {
      id
      courses {
        code
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
  "createStudents": {
    "students": [
      { "id": "s1", "courses": [{ "code": "CS101" }, { "code": "MA201" }] }
    ],
    "info": { "nodesCreated": 1, "relationshipsCreated": 2 }
  }
}
```

The student and both relationships are written in one transaction. Had
`MA201` not existed, the whole mutation would have failed with
`NOT_FOUND` (`Student.courses: no Course with code "MA201"`) and written
nothing, which is the foreign key check.

### Create both sides at once

`create` makes the related node and the link together:

```graphql
mutation {
  createStudents(
    input: [
      {
        id: "s2"
        name: "Grace"
        courses: {
          connect: [{ code: "CS101", edge: { grade: 7.4, role: "auditor" } }]
          create: [
            {
              node: { code: "EE150", title: "Circuits", credits: 5 }
              edge: { grade: 8 }
            }
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
  "createStudents": {
    "info": { "nodesCreated": 2, "relationshipsCreated": 2 }
  }
}
```

Two nodes (`Grace` and `EE150`) and two relationships.

### Link from either side

The link belongs to neither type, so either one can write it. This adds the
same kind of relationship starting from the course:

```graphql
mutation {
  updateCourse(
    code: "PH110"
    update: { students: { connect: [{ id: "s2" }] } }
  ) {
    course {
      students {
        name
      }
    }
  }
}
```

```json
{ "updateCourse": { "course": { "students": [{ "name": "Grace" }] } } }
```

### One link per pair

`connect` on a pair that is already connected does not add a second
relationship. It sets the properties you pass and keeps the others. Ada is
already enrolled in `CS101`, so connecting it again with a regraded exam
changes the grade and `relationshipsCreated` reports `0`:

```graphql
mutation {
  updateStudent(
    id: "s1"
    update: {
      courses: { connect: [{ code: "CS101", edge: { grade: 9.4 } }] }
    }
  ) {
    info {
      relationshipsCreated
    }
  }
}
```

```json
{ "updateStudent": { "info": { "relationshipsCreated": 0 } } }
```

That is the composite primary key, with upsert behaviour instead of a
duplicate key error. It makes `connect` safe to retry.

### Change or remove a link

One `update` input can change link data, add links and remove links, all
in one transaction. Grace gets a new grade for `CS101`, joins `MA201` and
drops `PH110`:

```graphql
mutation {
  updateStudent(
    id: "s2"
    update: {
      courses: {
        update: [{ code: "CS101", edge: { grade: 7.9 } }]
        connect: [{ code: "MA201" }]
        disconnect: ["PH110"]
      }
    }
  ) {
    info {
      nodesUpdated
      relationshipsCreated
      relationshipsDeleted
    }
  }
}
```

```json
{
  "updateStudent": {
    "info": {
      "nodesUpdated": 1,
      "relationshipsCreated": 1,
      "relationshipsDeleted": 1
    }
  }
}
```

| SQL | Nested input | Notes |
|---|---|---|
| `INSERT INTO enrollments` | `connect` | Fails with `NOT_FOUND` when the target does not exist |
| `UPDATE enrollments SET grade = ...` | `update: [{ code, edge }]` | Fails with `NOT_FOUND` when the pair is not connected |
| `DELETE FROM enrollments` | `disconnect: [keys]` | A pair that is not connected is ignored |
| `INSERT` into both tables | `create: [{ node, edge }]` | |
| `DELETE FROM courses` | `delete: { where }` | Deletes the related **nodes** |

:::warning disconnect and delete are different
`disconnect` removes the link and leaves both nodes. `delete` removes the
related nodes themselves, here the courses, for every student. To drop an
enrollment you want `disconnect`.
:::

To stop clients from doing some of these through a field, list the ones
you allow: `@relationship(..., nestedOperations: [CONNECT, DISCONNECT])`.

## Read the links

### From both sides

```graphql
{
  student(id: "s1") {
    name
    courses(sort: [{ title: ASC }]) {
      code
      title
    }
  }
  course(code: "CS101") {
    title
    students {
      name
    }
  }
}
```

```json
{
  "student": {
    "name": "Ada",
    "courses": [
      { "code": "CS101", "title": "Algorithms" },
      { "code": "MA201", "title": "Linear Algebra" }
    ]
  },
  "course": {
    "title": "Algorithms",
    "students": [{ "name": "Ada" }, { "name": "Grace" }]
  }
}
```

Each root field is one Cypher statement. Here is the first, where the SQL
version would join `students`, `enrollments` and `courses`:

```cypher
MATCH (this:Student)
WHERE this.id = $p0
CALL {
  WITH this
  MATCH (this)-[:ENROLLED_IN]->(this_courses:Course)
  WITH this_courses ORDER BY this_courses.title ASC LIMIT $p1
  RETURN collect(this_courses { .code, .title }) AS this_courses_list
}
RETURN this { .name, courses: this_courses_list } AS this
```

Nesting further (students, their courses, those courses' students) stays
one statement, with every level collected and bounded on its own.

:::note Lists are bounded and nesting is costed
A relationship list returns at most 25 nodes unless you pass `limit`, and
never more than 100. That holds for nested lists too. To walk a long list,
page the connection below.

Before an operation runs, the library estimates the rows it could touch by
multiplying the page sizes down the tree, and refuses with `COST_EXCEEDED`
above `maxCost` (50,000 by default). With no other information it assumes
every parent has a full page of 25, so four levels of the example
(students, courses, students, courses) estimate about 406,900 rows and are
refused, while three levels pass. Two things bring the estimate down to
what the data really holds: explicit `limit` arguments, and
`@cardinality(max:)` on the relationship field. With
`courses: [Course!]! @cardinality(max: 8)` and `students` at `max: 40`, the
same four levels estimate 45,225 rows and run. See
[statistics and cost](/docs/graphql/smart-layer#s6-statistics-and-cost).
:::

### The join columns

The plain list returns the related nodes. To read the data on the link
(`grade`, `role`, `enrolledAt`), use the field's connection: every list
relationship `x` also has `xConnection`, and each edge has the node plus
the relationship's `properties`.

```graphql
{
  student(id: "s1") {
    coursesConnection(first: 10, sort: [{ edge: { grade: DESC } }]) {
      totalCount
      edges {
        properties {
          grade
          role
          enrolledAt
        }
        node {
          code
          title
        }
      }
    }
  }
}
```

```json
{
  "student": {
    "coursesConnection": {
      "totalCount": 2,
      "edges": [
        {
          "properties": {
            "grade": null,
            "role": "student",
            "enrolledAt": "2026-10-07T09:14:22.242925000Z"
          },
          "node": { "code": "MA201", "title": "Linear Algebra" }
        },
        {
          "properties": {
            "grade": 9.4,
            "role": "student",
            "enrolledAt": "2026-10-07T09:14:22.242874000Z"
          },
          "node": { "code": "CS101", "title": "Algorithms" }
        }
      ]
    }
  }
}
```

`role` came from `@default` and `enrolledAt` from `@timestamp`; neither was
in the input. Descending order puts nulls first, as in the rest of the API.

### Filter and sort by the join columns

A relationship property marked `@filterable` can be filtered under `edge`
in the connection's `where`, next to `node` for the related node. One
marked `@sortable` can be sorted under `edge`, as above.

```graphql
{
  student(id: "s1") {
    coursesConnection(
      where: { edge: { grade: { gte: 8 } }, node: { credits: { gte: 5 } } }
    ) {
      edges {
        properties {
          grade
        }
        node {
          code
        }
      }
    }
  }
}
```

### Page a long list

Connections page by keyset cursor, including when the sort is on a
relationship property:

```graphql
query ($after: String) {
  student(id: "s1") {
    coursesConnection(
      first: 2
      after: $after
      sort: [{ edge: { grade: DESC } }]
    ) {
      edges {
        properties {
          grade
        }
        node {
          code
        }
      }
      pageInfo {
        hasNextPage
        endCursor
      }
    }
  }
}
```

Pass `pageInfo.endCursor` as `$after` for the next page. There is no
offset: a cursor records the sort values of the last row, so a page costs
the same however deep it is.

## Filter one side by the other

`@filterable` on a relationship field lets clients filter the parent by
what it is connected to. This is the `WHERE EXISTS (SELECT ... FROM
enrollments ...)` family.

```graphql
{
  students(where: { courses: { some: { code: { eq: "CS101" } } } }) {
    name
  }
}
```

```json
{ "students": [{ "name": "Ada" }, { "name": "Grace" }] }
```

| Filter | Matches students where | SQL equivalent |
|---|---|---|
| `some: {...}` | at least one course matches | `EXISTS` |
| `none: {...}` | no course matches | `NOT EXISTS` |
| `all: {...}` | every course matches | `NOT EXISTS (... WHERE NOT ...)` |
| `single: {...}` | exactly one course matches | `(SELECT count(*) ...) = 1` |
| `count: { gte: 2 }` | the number of courses compares | `(SELECT count(*) ...) >= 2` |
| `aggregate: {...}` | an aggregate of the courses or links compares | `HAVING` |

`all` is true for a student with no courses, as it is in SQL. Add
`count: { gte: 1 }` if you mean "has courses and all of them match".

To put a condition on the link as well as the node, use the connection
form of the filter. This finds students with a grade of 9 or higher in
CS101 specifically, not a 9 in something and CS101 somewhere else:

```graphql
{
  students(
    where: {
      coursesConnection: {
        some: { node: { code: { eq: "CS101" } }, edge: { grade: { gte: 9 } } }
      }
    }
  ) {
    name
  }
}
```

```json
{ "students": [{ "name": "Ada" }] }
```

Aggregate filters are the `GROUP BY ... HAVING` queries. Students whose
average grade is at least 7.5, and courses with at least two students:

```graphql
{
  students(
    where: {
      courses: { aggregate: { edge: { grade: { avg: { gte: 7.5 } } } } }
    }
  ) {
    name
  }
  courses(where: { students: { count: { gte: 2 } } }) {
    code
  }
}
```

```json
{
  "students": [{ "name": "Ada" }, { "name": "Grace" }],
  "courses": [{ "code": "CS101" }, { "code": "MA201" }]
}
```

A `some` filter with an equality on an indexed field does not scan the
students. The compiler starts from the matching course and walks back
along the relationship:

```cypher
MATCH (this_courses_anchor:Course)
WHERE this_courses_anchor.code = $p2
MATCH (this_courses_anchor)<-[:ENROLLED_IN]-(this:Student)
...
```

`lora-graphql check` plans every operation in your operation files and
fails when one would scan a label instead. See
[plan checks](/docs/graphql/smart-layer#s2-plan-checks).

## Totals

The connection's `aggregate` field computes over every related node and
link, not just the page:

```graphql
{
  student(id: "s1") {
    coursesConnection {
      aggregate {
        count {
          nodes
          edges
        }
        edge {
          grade {
            avg
            max
          }
        }
        node {
          credits {
            sum
          }
        }
      }
    }
  }
}
```

```json
{
  "student": {
    "coursesConnection": {
      "aggregate": {
        "count": { "nodes": 2, "edges": 2 },
        "edge": { "grade": { "avg": 9.4, "max": 9.4 } },
        "node": { "credits": { "sum": 11 } }
      }
    }
  }
}
```

Aggregates skip nulls, so the average is over the one graded course.
`count.nodes` and `count.edges` differ only when two nodes are linked more
than once, which the generated mutations never do (see
[one link per pair](#one-link-per-pair)); they can differ on data written
with raw Cypher.

`@query(aggregate: true)` on a type adds a root aggregate that takes the
same filters, so "how many students take CS101" is:

```graphql
{
  studentsAggregate(where: { courses: { some: { code: { eq: "CS101" } } } }) {
    count
  }
}
```

```json
{ "studentsAggregate": { "count": 2 } }
```

## Deleting a side

By default, deleting a node removes its relationships, so no link is left
pointing at nothing. The nodes on the other side stay:

```graphql
mutation {
  deleteStudent(id: "s2") {
    nodesDeleted
    relationshipsDeleted
  }
}
```

```json
{ "deleteStudent": { "nodesDeleted": 1, "relationshipsDeleted": 3 } }
```

This is `ON DELETE CASCADE` on the join table's foreign keys. It is the
default (`onDelete: DETACH`) because a graph cannot hold a relationship
without both ends.

For a true many-to-many that is all you want. The other two settings of
`@relationship(onDelete:)` cover the other foreign key behaviours:

- `CASCADE` deletes the related nodes too. Use it where one side owns the
  other, such as an order and its lines.
- `RESTRICT` refuses the delete while related nodes exist, which is a
  foreign key without `ON DELETE`.

## A table related to itself

A self-referential many-to-many (`follows (follower_id, followed_id)`) is
two fields on one type, one per direction:

```graphql
type Person @node @mutation {
  handle: String! @key
  follows: [Person!]!
    @relationship(type: "FOLLOWS", direction: OUT, properties: "Follow")
    @filterable
  followers: [Person!]!
    @relationship(type: "FOLLOWS", direction: IN, properties: "Follow")
    @filterable
  friends: [Person!]!
    @relationship(
      type: "FRIEND_OF"
      direction: OUT
      queryDirection: UNDIRECTED
    )
}

type Follow @relationshipProperties {
  since: Date
}
```

After `ada` follows `grace`, `grace.followers` contains `ada` and
`ada.follows` contains `grace`: one relationship, read from each end.

`friends` is a symmetric relationship. In SQL you either store two rows per
friendship or query with `OR` on both columns. Here
`queryDirection: UNDIRECTED` stores one relationship and reads it from
both ends: connect `ada` to `linus` once and each lists the other as a
friend.

## When the join row should be a node

A relationship connects exactly two nodes, and the API keeps one per pair.
Promote the join row to a node of its own when any of these is true:

- **A pair can be linked more than once.** A student who retakes a course
  has two enrollments.
- **The link involves a third thing.** An enrollment in a term, an
  assignment of a person to a project in a role.
- **The link has relationships of its own.** An enrollment graded by a
  teacher.
- **The link needs its own identity or API.** A key clients can address, a
  root field, its own mutations, subscriptions or authorization rules.

This is the same step as giving a join table its own surrogate key in SQL,
and the schema then has three node types and no relationship properties:

```graphql title="schema.graphql"
type Student @node @mutation {
  id: ID! @key
  name: String!
  enrollments: [Enrollment!]!
    @relationship(type: "HAS_ENROLLMENT", direction: OUT, onDelete: CASCADE)
    @filterable
}

type Course @node @mutation {
  code: String! @key
  title: String!
  enrollments: [Enrollment!]!
    @relationship(type: "FOR_COURSE", direction: IN)
    @filterable
}

type Term @node @mutation(operations: [CREATE, DELETE]) {
  code: String! @key
}

type Enrollment
  @node
  @mutation
  @query(aggregate: true)
  @uniqueTogether(fields: ["student", "course", "term"]) {
  id: ID! @key(generate: true)
  grade: Float @filterable(byValue: [GTE]) @sortable
  student: Student!
    @relationship(type: "HAS_ENROLLMENT", direction: IN)
    @filterable
  course: Course!
    @relationship(type: "FOR_COURSE", direction: OUT)
    @filterable
  term: Term! @relationship(type: "IN_TERM", direction: OUT) @filterable
}
```

What each relational constraint became:

- **`NOT NULL` on a foreign key** is a non-null single relationship
  (`student: Student!`). Creating an enrollment without one fails with
  `BAD_USER_INPUT`: `Enrollment.student is required: connect or create one`.
- **A composite unique constraint** is `@uniqueTogether`. A second
  enrollment for the same student, course and term fails with
  `CONSTRAINT_VIOLATION`; the same student and course in another term is
  fine.
- **`ON DELETE CASCADE`** is `onDelete: CASCADE` on `Student.enrollments`:
  deleting a student deletes their enrollments.

```graphql
mutation {
  createEnrollments(
    input: [
      {
        grade: 8.0
        student: { connect: { id: "s1" } }
        course: { connect: { code: "CS101" } }
        term: { connect: { code: "2026B" } }
      }
    ]
  ) {
    enrollments {
      id
    }
  }
}
```

Reads go through the enrollment in either direction, and the enrollment is
filterable by everything it connects:

```graphql
{
  student(id: "s1") {
    enrollments(sort: [{ grade: DESC }]) {
      grade
      course {
        title
      }
      term {
        code
      }
    }
  }
  enrollments(
    where: {
      course: { code: { eq: "CS101" } }
      term: { code: { eq: "2026B" } }
    }
  ) {
    grade
    student {
      name
    }
  }
}
```

The cost is one more hop per read and one more node per link. Start with
relationship properties and promote when one of the four conditions shows
up. The promotion changes the API clients see (fields are removed and
their types change), so plan it as a breaking change and run
`lora-graphql diff` on the two schemas to list exactly what breaks.

## Who may link

By default any caller that can update a student can connect and disconnect
its courses. Rules on the relationship field decide who may write a link
and which links a caller may read, and they are compiled into the same
statements. See
[rules on relationship fields](/docs/graphql/authorization#relationship-rules).

## Checklist

1. One `@node` type per entity table, with its primary key as `@key`.
2. One relationship type per join table. Declare a list field on both
   types with the same `type`, opposite `direction`, and the same
   `properties`.
3. Join columns go in a `@relationshipProperties` type. Mark the ones
   clients filter or sort by.
4. Add `@filterable` to the relationship fields clients filter through.
5. Add `@cardinality(max:)` to list relationships with a known bound, or
   run `analyze()`, so cost estimates reflect the real fan-out.
6. Read plain lists for the nodes, `xConnection` for the join columns,
   totals and paging.
7. Write with `connect`, `disconnect` and `update`. Keep `delete` for
   owned nodes.
8. Promote the link to a node when a pair can repeat, a third entity is
   involved, or the link needs its own identity.

## See also

- [Relationships](/docs/graphql/relationships): every `@relationship`
  argument and generated type.
- [The generated API](/docs/graphql/generated-api): lists, connections,
  filters and mutations for every type.
- [Directive reference](/docs/graphql/directives)
- [Concepts: relationships](/docs/concepts/relationships): how the engine
  stores them.
