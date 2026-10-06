// The rule forms (`isViewer`, `viewer`, named rules, masks, relationship
// rules) on the surfaces beyond lists, lookups and mutations: counts and
// aggregates, search, nested relationship filters and subscriptions. Each
// must show a caller exactly what a plain list would.

import { parse, subscribe, type ExecutionResult } from "graphql";
import { describe, expect, test } from "vitest";
import { createTestLoraGraphQL, type TestLoraGraphQL } from "../src/testing.js";

const typeDefs = `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
}
extend schema
  @authorizationRules(rules: [{ name: "admin", where: { jwt: { roles: { includes: "admin" } } } }])

type Person @node @query(aggregate: true) {
  key: String! @key
  name: String! @sortable
  verified: Boolean
  email: String @filterable
    @authorization(mask: [{ unless: { node: { isViewer: true } } }])
  trips: [Trip!]! @relationship(type: "MEMBER", direction: OUT, properties: "Membership") @filterable
}
type Membership @relationshipProperties {
  role: String @filterable @sortable
  seat: Int @filterable
}
type Trip @node @mutation @subscription(relationships: true) @query(aggregate: true)
  @fulltext(indexes: [{ fields: ["title"] }])
  @authorizationRule(name: "member", where: { OR: [
    { node: { members: { some: { isViewer: true } } } }
    { node: { owner: { isViewer: true } } } ] })
  @authorization(filter: [{ operations: [READ, SUBSCRIBE], where: { OR: [{ rule: "member" }, { rule: "admin" }] } }]) {
  key: String! @key
  title: String! @sortable
  days: Int @filterable @sortable
  budget: Int @filterable
    @authorization(mask: [{ unless: { node: { owner: { isViewer: true } } }, value: 0 }])
  owner: Person! @relationship(type: "OWNS", direction: IN) @filterable
  members: [Person!]! @relationship(type: "MEMBER", direction: IN, properties: "Membership") @filterable
    @authorization(validate: [{ operations: [READ_EDGE], where: { OR: [{ source: { owner: { isViewer: true } } }, { target: { isViewer: true } }] } }])
  notes: [Note!]! @relationship(type: "ABOUT", direction: IN) @filterable
}
type Note @node @mutation @subscription @query(aggregate: true)
  @fulltext(indexes: [{ fields: ["body"] }])
  @authorization(filter: [{ operations: [READ, SUBSCRIBE], where: { AND: [{ node: { trip: { rule: "member" } } }, { viewer: { verified: { eq: true } } }] } }]) {
  key: String! @key
  body: String!
  stars: Int @filterable
  trip: Trip! @relationship(type: "ABOUT", direction: OUT) @filterable
}`;
const seed = [
  "CREATE (:Person {key: 'lou', name: 'Lou', verified: true, email: 'l@x'}), (:Person {key: 'bo', name: 'Bo', verified: false, email: 'b@x'}), (:Person {key: 'cy', name: 'Cy', verified: true, email: 'c@x'}), (:Person {key: 'dee', name: 'Dee', verified: true, email: 'd@x'})",
  "CREATE (:Trip {key: 't1', title: 'Oslo summer', days: 5, budget: 900}), (:Trip {key: 't2', title: 'Oslo winter', days: 3, budget: 400}), (:Trip {key: 't3', title: 'Rome spring', days: 9, budget: 100})",
  "MATCH (p:Person {key: 'lou'}), (t:Trip {key: 't1'}) CREATE (p)-[:OWNS]->(t)",
  "MATCH (p:Person {key: 'cy'}), (t:Trip {key: 't2'}) CREATE (p)-[:OWNS]->(t)",
  "MATCH (p:Person {key: 'dee'}), (t:Trip {key: 't3'}) CREATE (p)-[:OWNS]->(t)",
  "MATCH (p:Person {key: 'bo'}), (t:Trip {key: 't1'}) CREATE (p)-[:MEMBER {role: 'guest', seat: 2}]->(t)",
  "MATCH (p:Person {key: 'cy'}), (t:Trip {key: 't1'}) CREATE (p)-[:MEMBER {role: 'guest', seat: 3}]->(t)",
  "MATCH (p:Person {key: 'lou'}), (t:Trip {key: 't2'}) CREATE (p)-[:MEMBER {role: 'lead', seat: 1}]->(t)",
  "MATCH (t:Trip {key: 't1'}) CREATE (:Note {key: 'n1', body: 'oslo packing list', stars: 5})-[:ABOUT]->(t)",
  "MATCH (t:Trip {key: 't2'}) CREATE (:Note {key: 'n2', body: 'oslo skis', stars: 2})-[:ABOUT]->(t)",
  "MATCH (t:Trip {key: 't3'}) CREATE (:Note {key: 'n3', body: 'pasta', stars: 4})-[:ABOUT]->(t)",
];

// lou owns t1 and is a member of t2; bo (unverified) and cy are members of
// t1; cy owns t2; dee owns t3 and is in no other trip. A note is readable
// by a verified member of its trip; a budget only by the trip's owner.
const as = (sub: string, roles: string[] = []) => ({ jwt: { sub, roles } });
const admin = as("root", ["admin"]);
const MASKED_EDGE = (field: string) =>
  `${field}: its READ_EDGE rules decide per relationship, so its properties cannot be filtered, sorted or aggregated by`;

type Context = Record<string, unknown> | undefined;
const messages = (r: ExecutionResult) => r.errors?.map((e) => e.message);

describe("counts and aggregates", () => {
  const counts = async (t: TestLoraGraphQL, context: Context) => {
    const d = await t.data<{
      tripsConnection: { totalCount: number };
      tripsAggregate: { count: number; days: { sum: number } };
      notesAggregate: { count: number };
    }>(
      "{ tripsConnection { totalCount } tripsAggregate { count days { sum } } notesAggregate { count } }",
      {},
      context,
    );
    return [
      d.tripsConnection.totalCount,
      d.tripsAggregate.count,
      d.tripsAggregate.days.sum,
      d.notesAggregate.count,
    ];
  };

  test("count what the named rule, isViewer and viewer let the caller read", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    expect(await counts(t, as("lou"))).toEqual([2, 2, 8, 2]);
    // A member, but not verified: the trip, none of its notes.
    expect(await counts(t, as("bo"))).toEqual([1, 1, 5, 0]);
    expect(await counts(t, as("dee"))).toEqual([1, 1, 9, 1]);
    // The admin rule opens trips; notes need the caller's own node.
    expect(await counts(t, admin)).toEqual([3, 3, 17, 0]);
    expect(await counts(t, undefined)).toEqual([0, 0, 0, 0]);
  });

  test("through a relationship, count only related nodes the caller reads", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const q =
      '{ person(key: "cy") { tripsConnection { totalCount aggregate { node { days { max } } } } } }';
    type R = {
      person: {
        tripsConnection: {
          totalCount: number;
          aggregate: { node: { days: { max: number | null } } };
        };
      };
    };
    expect((await t.data<R>(q, {}, as("bo"))).person.tripsConnection).toEqual({
      totalCount: 1,
      aggregate: { node: { days: { max: 5 } } },
    });
    expect((await t.data<R>(q, {}, as("dee"))).person.tripsConnection).toEqual({
      totalCount: 0,
      aggregate: { node: { days: { max: null } } },
    });
  });

  test("a field masked per row is not aggregated", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    expect(
      messages(
        await t.run(
          "{ tripsConnection { aggregate { node { budget { sum } } } } }",
          {},
          as("lou"),
        ),
      ),
    ).toEqual(["cannot aggregate Trip.budget: it is masked per row"]);
    expect(
      messages(
        await t.run(
          "{ personsConnection { aggregate { node { email { min } } } } }",
          {},
          as("lou"),
        ),
      ),
    ).toEqual(["cannot aggregate Person.email: it is masked per row"]);
  });

  test("properties under a READ_EDGE rule are not aggregated, filtered or sorted by", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    // Even for the owner, who reads every edge: the rule decides per row.
    for (const args of [
      "{ totalCount aggregate { edge { seat { sum } } } }",
      "(where: { edge: { seat: { eq: 3 } } }) { totalCount }",
      "(sort: [{ edge: { role: ASC } }]) { totalCount }",
    ]) {
      for (const who of ["lou", "bo"]) {
        expect(
          messages(
            await t.run(
              `{ trip(key: "t1") { membersConnection${args} } }`,
              {},
              as(who),
            ),
          ),
        ).toEqual([MASKED_EDGE("Trip.members")]);
      }
    }
  });

  test("an edge's properties read per relationship", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const q =
      '{ trip(key: "t1") { membersConnection { edges { node { key } properties { seat } } } } }';
    const owner = await t.run(q, {}, as("lou"));
    expect(owner.errors).toBeUndefined();
    expect(owner.data).toEqual({
      trip: {
        membersConnection: {
          edges: [
            { node: { key: "bo" }, properties: { seat: 2 } },
            { node: { key: "cy" }, properties: { seat: 3 } },
          ],
        },
      },
    });
    // A member reads their own seat; the other's is refused, not shown.
    const member = await t.run(q, {}, as("bo"));
    expect(messages(member)).toEqual(["not allowed to read Membership.seat"]);
    expect(member.data).toEqual({
      trip: {
        membersConnection: {
          edges: [
            { node: { key: "bo" }, properties: { seat: 2 } },
            { node: { key: "cy" }, properties: { seat: null } },
          ],
        },
      },
    });
  });
});

describe("search", () => {
  const found = async (t: TestLoraGraphQL, context: Context) => {
    const d = await t.data<{
      searchTrips: Array<{ node: { key: string; budget: number } }>;
      searchTripsConnection: { totalCount: number };
      searchNotes: Array<{ node: { key: string } }>;
      searchNotesConnection: { totalCount: number };
    }>(
      `{ searchTrips(query: "oslo") { node { key budget } }
         searchTripsConnection(query: "oslo") { totalCount }
         searchNotes(query: "oslo") { node { key } }
         searchNotesConnection(query: "oslo") { totalCount } }`,
      {},
      context,
    );
    return {
      trips: Object.fromEntries(
        d.searchTrips.map((m) => [m.node.key, m.node.budget]),
      ),
      tripCount: d.searchTripsConnection.totalCount,
      notes: d.searchNotes.map((m) => m.node.key).sort(),
      noteCount: d.searchNotesConnection.totalCount,
    };
  };

  test("matches and counts follow the rules; masks apply to the matches", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    expect(await found(t, as("lou"))).toEqual({
      trips: { t1: 900, t2: 0 },
      tripCount: 2,
      notes: ["n1", "n2"],
      noteCount: 2,
    });
    expect(await found(t, as("bo"))).toEqual({
      trips: { t1: 0 },
      tripCount: 1,
      notes: [],
      noteCount: 0,
    });
    expect(await found(t, as("cy"))).toEqual({
      trips: { t1: 0, t2: 400 },
      tripCount: 2,
      notes: ["n1", "n2"],
      noteCount: 2,
    });
    const nothing = { trips: {}, tripCount: 0, notes: [], noteCount: 0 };
    expect(await found(t, as("dee"))).toEqual(nothing);
    expect(await found(t, undefined)).toEqual(nothing);
    expect(await found(t, admin)).toEqual({
      trips: { t1: 0, t2: 0 },
      tripCount: 2,
      notes: [],
      noteCount: 0,
    });
  });

  test("where compares the masked value, on the match and through a relationship", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const keys = async (where: string, who: string) =>
      (
        await t.data<{ searchTrips: Array<{ node: { key: string } }> }>(
          `{ searchTrips(query: "oslo", where: ${where}) { node { key } } }`,
          {},
          as(who),
        )
      ).searchTrips.map((m) => m.node.key);
    expect(await keys("{ budget: { eq: 400 } }", "lou")).toEqual([]);
    expect(await keys("{ budget: { eq: 400 } }", "cy")).toEqual(["t2"]);
    const byOwner = '{ owner: { email: { eq: "c@x" } } }';
    expect(await keys(byOwner, "lou")).toEqual([]);
    expect(await keys(byOwner, "cy")).toEqual(["t2"]);
  });
});

describe("nested relationship filters", () => {
  const keys = async (
    t: TestLoraGraphQL,
    field: "persons" | "trips" | "notes",
    where: string,
    who: string,
  ) =>
    (
      await t.data<Record<string, Array<{ key: string }>>>(
        `{ ${field}(where: ${where}) { key } }`,
        {},
        as(who),
      )
    )
      [field]!.map((n) => n.key)
      .sort();

  test("a related node the caller can't read matches nothing", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    // lou is on t2, which bo cannot read: the filter must not say so.
    const onT2 = '{ trips: { some: { key: { eq: "t2" } } } }';
    expect(await keys(t, "persons", onT2, "bo")).toEqual([]);
    expect(await keys(t, "persons", onT2, "cy")).toEqual(["lou"]);
    // Notes need a verified viewer: bo's trip has one he cannot see.
    const starred = "{ notes: { some: { stars: { eq: 5 } } } }";
    expect(await keys(t, "trips", starred, "bo")).toEqual([]);
    expect(await keys(t, "trips", starred, "cy")).toEqual(["t1"]);
  });

  test("a masked field of a related node compares as the caller reads it", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const budget = (n: number) => `{ trip: { budget: { eq: ${n} } } }`;
    expect(await keys(t, "notes", budget(400), "lou")).toEqual([]);
    expect(await keys(t, "notes", budget(0), "lou")).toEqual(["n2"]);
    expect(await keys(t, "notes", budget(400), "cy")).toEqual(["n2"]);
    expect(await keys(t, "notes", budget(0), "cy")).toEqual(["n1"]);
    const owner = '{ owner: { email: { eq: "l@x" } } }';
    expect(await keys(t, "trips", owner, "bo")).toEqual([]);
    expect(await keys(t, "trips", owner, "lou")).toEqual(["t1"]);
    const member = '{ members: { some: { email: { eq: "c@x" } } } }';
    expect(await keys(t, "trips", member, "bo")).toEqual([]);
    expect(await keys(t, "trips", member, "cy")).toEqual(["t1"]);
  });

  test("edge properties under a READ_EDGE rule are not filtered by, from either side", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const edge = "{ some: { edge: { seat: { eq: 3 } } } }";
    for (const who of ["lou", "bo", "cy"]) {
      expect(
        messages(
          await t.run(
            `{ trips(where: { membersConnection: ${edge} }) { key } }`,
            {},
            as(who),
          ),
        ),
      ).toEqual([MASKED_EDGE("Trip.members")]);
      // The rule is declared on Trip.members; Person.trips is the same edge.
      expect(
        messages(
          await t.run(
            `{ persons(where: { tripsConnection: ${edge} }) { key } }`,
            {},
            as(who),
          ),
        ),
      ).toEqual([MASKED_EDGE("Person.trips")]);
    }
  });
});

describe("subscriptions", () => {
  const settle = () => new Promise((r) => setTimeout(r, 40));
  async function listen(t: TestLoraGraphQL, query: string, context: Context) {
    const controller = new AbortController();
    const result = await subscribe({
      schema: t.schema,
      document: parse(query),
      contextValue: { ...context, signal: controller.signal },
    });
    if (!(Symbol.asyncIterator in result)) throw result.errors?.[0];
    const events: unknown[] = [];
    const done = (async () => {
      for await (const r of result as AsyncIterable<ExecutionResult>) {
        events.push(r.errors ? { errors: messages(r) } : r.data);
      }
    })();
    return {
      events,
      stop: async () => {
        controller.abort();
        await (result as AsyncGenerator).return?.(undefined);
        await done;
      },
    };
  }
  /** Subscribe each caller, run the writes, and return what each saw. */
  async function watch(
    t: TestLoraGraphQL,
    query: string,
    callers: Record<string, Context>,
    writes: Array<[string, Context]>,
  ) {
    const listeners = await Promise.all(
      Object.entries(callers).map(
        async ([who, context]) =>
          [who, await listen(t, query, context)] as const,
      ),
    );
    for (const [mutation, context] of writes) {
      await t.data(mutation, {}, context);
    }
    await settle();
    const seen: Record<string, unknown[]> = {};
    for (const [who, l] of listeners) {
      await l.stop();
      seen[who] = l.events;
    }
    return seen;
  }
  const retitle = (key: string, title: string) =>
    `mutation { updateTrip(key: "${key}", update: { title: "${title}" }) { trip { key } } }`;

  test("events reach the callers the named rule lets read, masked for each", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const seen = await watch(
      t,
      "subscription { tripChanged { key node { title budget owner { key email } } } }",
      {
        lou: as("lou"),
        bo: as("bo"),
        dee: as("dee"),
        admin,
        anonymous: undefined,
      },
      [[retitle("t1", "Oslo midsummer"), as("lou")]],
    );
    const event = (budget: number, email: string | null) => [
      {
        tripChanged: {
          key: "t1",
          node: {
            title: "Oslo midsummer",
            budget,
            owner: { key: "lou", email },
          },
        },
      },
    ];
    expect(seen).toEqual({
      lou: event(900, "l@x"),
      bo: event(0, null),
      dee: [],
      admin: event(0, null),
      anonymous: [],
    });
  });

  test("a caller a write makes a member hears of it from then on", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const seen = await watch(
      t,
      "subscription { tripChanged { operation key relationship { field relatedKey } } }",
      { dee: as("dee") },
      [
        [retitle("t1", "Before"), as("lou")],
        [
          'mutation { updateTrip(key: "t1", update: { members: { connect: [{ key: "dee" }] } }) { trip { key } } }',
          as("lou"),
        ],
      ],
    );
    expect(seen["dee"]).toEqual([
      { tripChanged: { operation: "UPDATE", key: "t1", relationship: null } },
      {
        tripChanged: {
          operation: "CONNECT",
          key: "t1",
          relationship: { field: "Trip.members", relatedKey: "dee" },
        },
      },
    ]);
  });

  test("viewer: an unverified member hears nothing of the notes", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const seen = await watch(
      t,
      "subscription { noteChanged { key node { body trip { key budget } } } }",
      { lou: as("lou"), bo: as("bo"), cy: as("cy"), dee: as("dee"), admin },
      [
        [
          'mutation { updateNote(key: "n1", update: { body: "oslo list" }) { note { key } } }',
          as("lou"),
        ],
      ],
    );
    const event = (budget: number) => [
      {
        noteChanged: {
          key: "n1",
          node: { body: "oslo list", trip: { key: "t1", budget } },
        },
      },
    ];
    expect(seen).toEqual({
      lou: event(900),
      bo: [],
      cy: event(0),
      dee: [],
      admin: [],
    });
  });

  test("where filters on what the subscriber reads, related nodes included", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const one = [{ tripChanged: { key: "t1" } }];
    for (const [where, callers] of [
      // bo reads the budget as 0, and the owner's email as null.
      ["{ budget: { eq: 900 } }", { lou: one, bo: [] }],
      ['{ owner: { email: { eq: "l@x" } } }', { lou: one, bo: [] }],
      // bo is a member, but cannot read the trip's notes; cy can.
      ["{ notes: { some: { stars: { eq: 5 } } } }", { cy: one, bo: [] }],
    ] as const) {
      const seen = await watch(
        t,
        `subscription { tripChanged(where: ${where}) { key } }`,
        Object.fromEntries(Object.keys(callers).map((who) => [who, as(who)])),
        [[retitle("t1", "Again"), as("lou")]],
      );
      expect(seen, where).toEqual(callers);
    }
  });
});
