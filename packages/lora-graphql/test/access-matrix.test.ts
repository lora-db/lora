// The access matrix (`lora.accessMatrix()`, `lora-graphql access`) and the
// authorization lints `check()` reports.

import { readFileSync } from "node:fs";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, test } from "vitest";
import { main } from "../src/cli.js";
import { LoraGraphQL, loraDriver } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";
import { createDatabase } from "@loradb/lora-node";

const readme = readFileSync(new URL("../README.md", import.meta.url), "utf8");
// The Post example of the README's authorization section.
const post =
  /```graphql\n(type Claims @jwt[\s\S]*?type Post[\s\S]*?)```/.exec(
    readme,
  )![1]! + "\ntype User @node { key: String! @key }";

const lora = async (typeDefs: string) =>
  new LoraGraphQL({ typeDefs, driver: loraDriver(await createDatabase()) });

describe("accessMatrix", () => {
  test("the README's Post example", async () => {
    const matrix = (await lora(post)).accessMatrix();
    expect(
      matrix.map(
        (e) =>
          `${e.field ? `${e.type}.${e.field}` : e.type} ${e.operation} ${e.principal}: ${e.verdict} (${e.by.join(", ")})`,
      ),
    ).toMatchSnapshot();
  });

  test("verdicts per kind of caller", async () => {
    const typeDefs = `type Claims @jwt { sub: String!  roles: [String!] }
extend schema @authorizationDefaults(bypass: { jwt: { roles: { includes: "admin" } } })
type Doc @node @mutation
  @authentication(operations: [CREATE, UPDATE, DELETE])
  @authorization(
    filter: [{ where: { node: { owner: { eq: "$jwt.sub" } } } }]
    validate: [{ operations: [CREATE], where: { jwt: { roles: { includes: "editor" } } } }]
  ) {
  key: String! @key
  owner: String!
  secret: String @authorization(mask: [{ unless: { node: { owner: { eq: "$jwt.sub" } } } }])
}`;
    const verdict = (m: ReturnType<LoraGraphQL["accessMatrix"]>, id: string) =>
      m.find(
        (e) =>
          `${e.field ? `${e.type}.${e.field}` : e.type} ${e.operation} ${e.principal}` ===
          id,
      )?.verdict;
    const m = (await lora(typeDefs)).accessMatrix();
    expect(verdict(m, "Doc READ anonymous")).toBe("denied");
    expect(verdict(m, "Doc READ authenticated")).toBe("filtered");
    expect(verdict(m, "Doc READ roles:admin")).toBe("allowed");
    expect(verdict(m, "Doc CREATE anonymous")).toBe("unauthenticated");
    expect(verdict(m, "Doc CREATE authenticated")).toBe("denied");
    expect(verdict(m, "Doc CREATE roles:editor")).toBe("allowed");
    expect(verdict(m, "Doc.secret READ authenticated")).toBe("masked");
    expect(verdict(m, "Doc.secret READ roles:admin")).toBe("allowed");
  });

  test("root @cypher fields, guarded or not", async () => {
    const typeDefs = `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
}
type Person @node { key: String! @key  verified: Boolean }
type Query {
  count: Int @cypher(statement: "MATCH (p:Person) RETURN count(p) AS n")
}
type Mutation {
  wipe: Int
    @authorization(validate: [{ where: { jwt: { roles: { includes: "admin" } } } }])
    @cypher(statement: "MATCH (p:Person) DETACH DELETE p RETURN 0 AS n")
  verify: Person
    @authentication
    @authorization(validate: [{ where: { viewer: { verified: { eq: true } } } }])
    @cypher(statement: "MATCH (p:Person) WHERE p.key = $viewer SET p.verified = true RETURN p")
  touch: Int @cypher(statement: "MERGE (p:Person {key: 'x'}) RETURN 1 AS n")
}`;
    const matrix = (await lora(typeDefs))
      .accessMatrix()
      .filter((e) => e.type === "Query" || e.type === "Mutation");
    expect(
      matrix.map(
        (e) =>
          `${e.type}.${e.field} ${e.operation} ${e.principal}: ${e.verdict} (${e.by.join(", ")})`,
      ),
    ).toMatchSnapshot();
  });

  test("relationship properties, field writes and key scopes", async () => {
    const typeDefs = `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
}
type Person @node @mutation {
  key: String! @key
  trips: [Trip!]! @relationship(type: "MEMBER", direction: OUT, properties: "E")
}
type E @relationshipProperties {
  rsvp: String
  marker: String
    @authentication(operations: [UPDATE])
    @authorization(validate: [
      { operations: [READ], where: { jwt: { roles: { includes: "staff" } } } }
      { operations: [CREATE], where: { jwt: { roles: { includes: "admin" } } } }
    ])
}
type Trip @node @mutation {
  key: String! @key(scope: VIEWER)
  name: String
  featured: Boolean
    @authorization(validate: [{ operations: [CREATE, UPDATE], where: { jwt: { roles: { includes: "admin" } } } }])
}`;
    const m = (await lora(typeDefs)).accessMatrix();
    const lines = m.map(
      (e) =>
        `${e.field ? `${e.type}.${e.field}` : e.type} ${e.operation} ${e.principal}: ${e.verdict} (${e.by.join(", ")})`,
    );
    expect(lines).toMatchSnapshot();
    const verdict = (id: string) =>
      m.find(
        (e) =>
          `${e.field ? `${e.type}.${e.field}` : e.type} ${e.operation} ${e.principal}` ===
          id,
      )?.verdict;
    expect(verdict("E.marker CREATE anonymous")).toBe("unauthenticated");
    expect(verdict("E.marker CREATE authenticated")).toBe("denied");
    expect(verdict("E.marker CREATE roles:admin")).toBe("allowed");
    expect(verdict("E.marker READ anonymous")).toBe("unauthenticated");
    expect(verdict("E.marker READ roles:staff")).toBe("allowed");
    expect(verdict("E.marker UPDATE anonymous")).toBe("unauthenticated");
    expect(verdict("E.marker UPDATE authenticated")).toBe("allowed");
    expect(verdict("E.rsvp READ anonymous")).toBeUndefined();
    expect(verdict("Trip CREATE anonymous")).toBe("unauthenticated");
    expect(verdict("Trip CREATE authenticated")).toBe("validated");
    expect(
      m.find((e) => e.type === "Trip" && e.operation === "CREATE")?.by,
    ).toEqual(["key scope"]);
    expect(verdict("Trip UPDATE anonymous")).toBe("allowed");
    expect(verdict("Trip.featured CREATE authenticated")).toBe("denied");
    expect(verdict("Trip.featured UPDATE roles:admin")).toBe("allowed");
  });

  test("viewer: the caller on nodes the rules name them for", async () => {
    const typeDefs = `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
}
type Person @node @mutation
  @authorization(validate: [{ operations: [UPDATE, DELETE], where: { node: { isViewer: true } } }]) {
  key: String! @key
  verified: Boolean
  email: String @authorization(mask: [{ unless: { node: { isViewer: true } } }])
  notes: String @authorization(mask: [{ unless: { node: { NOT: { isViewer: true } } } }])
}
type Trip @node @mutation
  @authorization(
    filter: [{ operations: [READ], where: { OR: [
      { node: { owner: { isViewer: true } } }
      { node: { members: { some: { isViewer: true } } } } ] } }]
    validate: [
      { operations: [UPDATE], where: { node: { owner: { isViewer: true }, open: { eq: true } } } }
      { operations: [DELETE], where: { AND: [{ node: { owner: { isViewer: true } } }, { viewer: { verified: { eq: true } } }] } }
    ]
  ) {
  key: String! @key
  open: Boolean
  owner: Person! @relationship(type: "OWNS", direction: IN)
  members: [Person!]! @relationship(type: "MEMBER", direction: IN)
    @authorization(validate: [{ operations: [DISCONNECT], where: { OR: [{ source: { owner: { isViewer: true } } }, { target: { isViewer: true } }] } }])
}`;
    const m = (await lora(typeDefs)).accessMatrix();
    const verdicts = (principal: string) =>
      Object.fromEntries(
        m
          .filter((e) => e.principal === principal)
          .map((e) => [
            `${e.field ? `${e.type}.${e.field}` : e.type} ${e.operation}`,
            e.verdict,
          ]),
      );
    expect(verdicts("authenticated")).toMatchObject({
      "Person UPDATE": "validated",
      "Person.email READ": "masked",
      "Trip READ": "filtered",
      "Trip.members DISCONNECT": "validated",
    });
    expect(verdicts("viewer")).toMatchObject({
      "Person UPDATE": "allowed",
      "Person DELETE": "allowed",
      "Person.email READ": "allowed",
      // Unless NOT them: on their own node the mask always applies.
      "Person.notes READ": "masked",
      "Trip READ": "allowed",
      // Their trip, but the rule also tests the trip and their own node.
      "Trip UPDATE": "validated",
      "Trip DELETE": "validated",
      "Trip.members DISCONNECT": "allowed",
    });
    // No rule names the caller's node: no such principal.
    const plain = (await lora(post)).accessMatrix();
    expect(plain.some((e) => e.principal === "viewer")).toBe(false);
  });

  test("@authentication on a relationship field guards connecting through it (G-43)", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: `type Claims @jwt { sub: String! }
extend schema @authorizationDefaults(requireAuthentication: false)
type Room @node @mutation(operations: [CREATE, UPDATE]) @authorization(public: [CREATE, UPDATE]) {
  key: String! @key
  kind: String!
  chan: Chan @relationship(type: "IN", direction: OUT, nestedOperations: [CONNECT, DISCONNECT])
    @authentication(operations: [CREATE_RELATIONSHIP])
    @authorization(validate: [
      { operations: [CONNECT], where: { source: { kind: { eq: "CHANNEL" } } } }
      { operations: [DISCONNECT], where: { source: { kind: { eq: "CHANNEL" } } } }
    ])
  # No rule beside it: the guard alone decides, for both operations.
  hall: Chan @relationship(type: "AT", direction: OUT, nestedOperations: [CONNECT, DISCONNECT])
    @authentication(operations: [CREATE_RELATIONSHIP, DELETE_RELATIONSHIP])
}
type Chan @node { key: String! @key }`,
    });
    await t.db.execute("CREATE (:Chan {key: 'c'})");
    const create = `mutation { createRooms(input: [{ key: "r", kind: "CHANNEL", chan: { connect: { key: "c" } } }]) { rooms { key } } }`;
    // What the matrix has to agree with: the request is refused.
    const anonymous = await t.run(create);
    expect(anonymous.errors?.[0]?.extensions?.["code"]).toBe("UNAUTHENTICATED");
    expect(await t.data(create, {}, { jwt: { sub: "u" } })).toEqual({
      createRooms: { rooms: [{ key: "r" }] },
    });

    const m = t.lora.accessMatrix();
    const row = (field: string, operation: string, principal: string) => {
      const e = m.find(
        (x) =>
          x.type === "Room" &&
          x.field === field &&
          x.operation === operation &&
          x.principal === principal,
      );
      return e && { verdict: e.verdict, by: e.by };
    };
    expect(row("chan", "CONNECT", "anonymous")).toEqual({
      verdict: "unauthenticated",
      by: ["@authentication", "validate[0]"],
    });
    expect(row("chan", "CONNECT", "authenticated")).toEqual({
      verdict: "validated",
      by: ["@authentication", "validate[0]"],
    });
    // DISCONNECT answers to DELETE_RELATIONSHIP, which `chan` does not list.
    expect(row("chan", "DISCONNECT", "anonymous")).toEqual({
      verdict: "validated",
      by: ["validate[1]"],
    });
    for (const op of ["CONNECT", "DISCONNECT"]) {
      expect(row("hall", op, "anonymous")).toEqual({
        verdict: "unauthenticated",
        by: ["@authentication"],
      });
      expect(row("hall", op, "authenticated")).toEqual({
        verdict: "allowed",
        by: ["@authentication"],
      });
    }
  });

  test("a relationship field's @authentication(jwt:) is asked like a type's (G-43)", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: `type Claims @jwt { sub: String!  roles: [String!] }
extend schema @authorizationDefaults(requireAuthentication: false)
type F @node @mutation(operations: [UPDATE]) @authorization(public: [UPDATE]) {
  key: String! @key
  score: Int @authentication(operations: [READ])
  tags: [T!]! @relationship(type: "TAGGED", direction: OUT, nestedOperations: [CONNECT, DISCONNECT])
    @authentication(operations: [CREATE_RELATIONSHIP, DELETE_RELATIONSHIP], jwt: { roles: { includes: "editor" } })
}
type T @node { key: String! @key }`,
    });
    await t.db.execute(
      "CREATE (:F {key: 'f'})-[:TAGGED]->(:T {key: 't'}), (:T {key: 'u'})",
    );
    const connect = `mutation { updateF(key: "f", update: { tags: { connect: [{ key: "u" }] } }) { f { key } } }`;
    const code = async (jwt?: Record<string, unknown>) =>
      (await t.run(connect, {}, jwt ? { jwt } : {})).errors?.[0]?.extensions?.[
        "code"
      ];
    // Enforcement, which the rows below have to match.
    expect(await code()).toBe("UNAUTHENTICATED");
    expect(await code({ sub: "u" })).toBe("UNAUTHENTICATED");
    expect(await code({ sub: "u", roles: ["editor"] })).toBeUndefined();

    const rows = t.lora
      .accessMatrix()
      .filter((e) => e.type === "F" && e.field !== undefined)
      .map((e) => `${e.field} ${e.operation} ${e.principal}: ${e.verdict}`);
    expect(rows).toEqual([
      // A scalar field's READ guard keeps its rows, before the
      // relationship field's, in field-name order.
      "score READ anonymous: unauthenticated",
      "score READ authenticated: allowed",
      "score READ roles:editor: allowed",
      // A token without the claim is refused like no token, as for a
      // type's @authentication(jwt:); the claim it asks for has a
      // principal of its own.
      "tags CONNECT anonymous: unauthenticated",
      "tags CONNECT authenticated: unauthenticated",
      "tags CONNECT roles:editor: allowed",
      "tags DISCONNECT anonymous: unauthenticated",
      "tags DISCONNECT authenticated: unauthenticated",
      "tags DISCONNECT roles:editor: allowed",
    ]);
  });

  test("viewer never reads below authenticated under NOT isViewer (G-44)", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: `type Claims @jwt { sub: String! @viewer(type: "P", field: "key") }
type P @node @mutation(operations: [UPDATE])
  @authorization(validate: [{ operations: [UPDATE], where: { AND: [
    { node: { isViewer: true } }
    { NOT: { node: { following: { some: { isViewer: true } } } } } ] } }]) {
  key: String! @key
  bio: String
  following: [P!]! @relationship(type: "FOLLOWS", direction: OUT)
}
type Q @node @mutation(operations: [UPDATE])
  @authorization(validate: [{ operations: [UPDATE], where: { NOT: { NOT: { node: { owner: { isViewer: true } } } } } }]) {
  key: String! @key
  owner: P! @relationship(type: "OWNS", direction: IN)
}`,
    });
    await t.db.execute("CREATE (:P {key: 'p'})");
    // The viewer does pass at run time.
    expect(
      await t.data(
        `mutation { updatePs(where: { key: { eq: "p" } }, update: { bio: "hi" }) { ps { bio } } }`,
        {},
        { jwt: { sub: "p" } },
      ),
    ).toEqual({ updatePs: { ps: [{ bio: "hi" }] } });

    const verdicts = (type: string) =>
      t.lora
        .accessMatrix()
        .filter((e) => e.type === type && e.operation === "UPDATE")
        .map((e) => [e.principal, e.verdict]);
    // Their own node passes; not following themselves is still per row.
    expect(verdicts("P")).toEqual([
      ["anonymous", "unauthenticated"],
      ["authenticated", "validated"],
      ["viewer", "validated"],
    ]);
    // Negated twice is not negated: the part passes as it does bare.
    expect(verdicts("Q")).toEqual([
      ["anonymous", "unauthenticated"],
      ["authenticated", "validated"],
      ["viewer", "allowed"],
    ]);
  });

  test("NOT isViewer inside OR / AND and in filter rules stays per row (G-44)", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: `type Claims @jwt { sub: String! @viewer(type: "P", field: "key") }
type P @node { key: String! @key }
type D @node @mutation(operations: [UPDATE, DELETE])
  @authorization(
    filter: [
      # Theirs, and not shared with themselves.
      { operations: [READ], where: { AND: [
        { node: { owner: { isViewer: true } } }
        { NOT: { node: { readers: { some: { isViewer: true } } } } } ] } }
      # The negation alone: nothing passes for being the viewer.
      { operations: [DELETE], where: { NOT: { node: { owner: { isViewer: true } } } } }
    ]
    validate: [
      # One branch is theirs outright, the other is a negation.
      { operations: [UPDATE], where: { OR: [
        { NOT: { node: { readers: { some: { isViewer: true } } } } }
        { AND: [{ node: { open: { eq: true } } }, { NOT: { node: { owner: { isViewer: true } } } }] } ] } }
      { operations: [DELETE], where: { OR: [
        { node: { owner: { isViewer: true } } }
        { NOT: { node: { owner: { isViewer: true } } } } ] } }
    ]
  ) {
  key: String! @key
  open: Boolean
  owner: P! @relationship(type: "OWNS", direction: IN)
  readers: [P!]! @relationship(type: "READS", direction: IN)
}`,
    });
    const m = t.lora.accessMatrix();
    const verdict = (operation: string, principal: string) =>
      m.find(
        (e) =>
          e.type === "D" &&
          e.field === undefined &&
          e.operation === operation &&
          e.principal === principal,
      )?.verdict;
    const rank = [
      "denied",
      "unauthenticated",
      "validated",
      "filtered",
      "allowed",
    ];
    for (const op of ["READ", "UPDATE", "DELETE"]) {
      // The claim itself: never below what any signed-in caller gets.
      expect(
        rank.indexOf(verdict(op, "viewer")!),
        `${op}: viewer ${verdict(op, "viewer")} vs authenticated ${verdict(op, "authenticated")}`,
      ).toBeGreaterThanOrEqual(rank.indexOf(verdict(op, "authenticated")!));
    }
    expect({
      READ: verdict("READ", "viewer"),
      UPDATE: verdict("UPDATE", "viewer"),
      DELETE: verdict("DELETE", "viewer"),
    }).toEqual({
      // The owner part passes; the negation still filters rows.
      READ: "filtered",
      // Both branches hang on a negation.
      UPDATE: "validated",
      // The filter is a bare negation: per row, as for anyone signed in.
      DELETE: "filtered",
    });
  });

  test("lora-graphql access prints it", async () => {
    const dir = await mkdtemp(join(tmpdir(), "lora-graphql-access-"));
    await writeFile(join(dir, "schema.graphql"), post);
    const out: string[] = [];
    const code = await main(["access", join(dir, "schema.graphql")], {
      out: (l) => out.push(l),
      err: (l) => out.push(l),
    });
    expect(code).toBe(0);
    expect(out.some((l) => /^Post\s+READ\s+anonymous\s+/.test(l))).toBe(true);
    const json: string[] = [];
    await main(["access", join(dir, "schema.graphql"), "--json"], {
      out: (l) => json.push(l),
      err: (l) => json.push(l),
    });
    expect(JSON.parse(json.join("\n"))).toEqual(
      (await lora(post)).accessMatrix(),
    );
  });
});

describe("authorization lints", () => {
  const lint = async (typeDefs: string) => {
    const t = await createTestLoraGraphQL({ typeDefs });
    return (await t.lora.check()).lint.map((w) =>
      w.field
        ? `${w.type}.${w.field}: ${w.message}`
        : `${w.type}: ${w.message}`,
    );
  };
  const claims = "type Claims @jwt { sub: String!  roles: [String!] }\n";

  test("a filter every signed-in caller passes", async () => {
    const found = await lint(
      claims +
        `type Doc @node @authorization(filter: [{ where: { jwt: { sub: { exists: true } } } }]) { key: String! @key }`,
    );
    expect(found).toContain(
      "Doc: filter[0] holds for every authenticated caller: it only keeps out anonymous ones (use @authentication for that)",
    );
  });

  test("requireAuthentication refusing a claim-free branch", async () => {
    const rule = `where: { OR: [{ node: { published: { eq: true } } }, { node: { owner: { eq: "$jwt.sub" } } }] }`;
    const typeDefs = (extra: string) =>
      claims +
      extra +
      `type Doc @node @authorization(filter: [{ ${rule} }]) { key: String! @key  published: Boolean!  owner: String! }`;
    expect(await lint(typeDefs(""))).toContain(
      "Doc: filter[0] has a branch that needs no claims (OR[0]), but requireAuthentication defaults to true, so anonymous callers are refused by the whole rule; set @authorizationDefaults(requireAuthentication: false) if anonymous callers should get such branches, or true to say they should not",
    );
    for (const said of [true, false]) {
      const extra = `extend schema @authorizationDefaults(requireAuthentication: ${said})\n`;
      expect(
        (await lint(typeDefs(extra))).some((l) =>
          l.includes("requireAuthentication defaults"),
        ),
      ).toBe(false);
    }
  });

  // Festimap G-45 / G-46: one schema, three settings of the default.
  const openOrOwned = (extra: string, typeLevel = "") =>
    `${extra}type Claims @jwt { sub: String! @viewer(type: "P", field: "key") }
type P @node { key: String! @key }
type F @node @mutation(operations: [CREATE, UPDATE]) ${typeLevel} @authorization(
  filter: [
    { operations: [READ], where: { node: { owner: { isViewer: true } } } }
    { operations: [READ], where: { node: { open: { eq: true } } } }]
  validate: [
    { operations: [CREATE], where: { node: { open: { eq: true } } } }
    { operations: [UPDATE], where: { OR: [{ node: { owner: { isViewer: true } } }, { node: { open: { eq: true } } }] } }]) {
  key: String! @key
  open: Boolean!
  owner: P @relationship(type: "OWNS", direction: IN)
  watchers: [P!]! @relationship(type: "WATCHES", direction: IN, nestedOperations: [CONNECT, DISCONNECT])
    @authorization(validate: [{ operations: [CONNECT, DISCONNECT], where: { source: { open: { eq: true } } } }])
}`;
  const off =
    "extend schema @authorizationDefaults(requireAuthentication: false)\n";

  test("a rule that needs no claims at all, while the default is unset (G-45)", async () => {
    const found = (await lint(openOrOwned(""))).filter((l) =>
      l.includes("needs no claims"),
    );
    const whole = (label: string) =>
      `${label} needs no claims, but requireAuthentication defaults to true, so anonymous callers are refused by it; set @authorizationDefaults(requireAuthentication: false) if anonymous callers should get such rules, or true to say they should not`;
    expect(found).toEqual([
      `F: ${whole("filter[1]")}`,
      `F: ${whole("validate[0]")}`,
      "F: validate[1] has a branch that needs no claims (OR[1]), but requireAuthentication defaults to true, so anonymous callers are refused by the whole rule; set @authorizationDefaults(requireAuthentication: false) if anonymous callers should get such branches, or true to say they should not",
      // On a field too: a CONNECT rule over `source` alone.
      `F.watchers: ${whole("validate[0]")}`,
    ]);
    // Said either way, the question is answered.
    for (const said of [true, false]) {
      const extra = `extend schema @authorizationDefaults(requireAuthentication: ${said})\n`;
      expect(
        (await lint(openOrOwned(extra))).filter((l) =>
          l.includes("needs no claims, but"),
        ),
      ).toEqual([]);
    }
  });

  test("the writes a signed-out caller passes once the default is false (G-46)", async () => {
    const signedOut = async (typeDefs: string) =>
      (await lint(typeDefs)).filter((l) => l.includes("signed-out caller"));
    const fix = (ops: string, guard: string, on = " to F") =>
      `lets a signed-out caller ${ops}: it needs no claims and requireAuthentication is false for the whole schema; test { jwt: { sub: { exists: true } } } beside it, or add @authentication(operations: [${guard}])${on}`;
    // READ rules are what the setting is for: silent.
    expect(await signedOut(openOrOwned(off))).toEqual([
      `F: validate[0] ${fix("CREATE", "CREATE")}`,
      `F: validate[1] ${fix("UPDATE", "UPDATE")}`,
      `F.watchers: validate[0] ${fix("CONNECT, DISCONNECT", "CREATE_RELATIONSHIP, DELETE_RELATIONSHIP", "")}`,
    ]);
    // What the lint describes does happen.
    const t = await createTestLoraGraphQL({ typeDefs: openOrOwned(off) });
    expect(
      await t.data(
        `mutation { createFs(input: [{ key: "n", open: true }]) { fs { key } } }`,
      ),
    ).toEqual({ createFs: { fs: [{ key: "n" }] } });

    // @authentication on the type covers its operations, and every
    // mutation a connect or disconnect can be written in.
    expect(
      await signedOut(
        openOrOwned(off, "@authentication(operations: [CREATE, UPDATE])"),
      ),
    ).toEqual([]);
    // Covering only the type's own writes leaves nothing to say about
    // them, and the relationship's operations guarded by name.
    expect(
      await signedOut(
        openOrOwned(
          off,
          "@authentication(operations: [CREATE, UPDATE, CREATE_RELATIONSHIP, DELETE_RELATIONSHIP])",
        ),
      ),
    ).toEqual([]);
    expect(
      await signedOut(
        openOrOwned(off, "@authentication(operations: [CREATE])"),
      ),
    ).toEqual([
      `F: validate[1] ${fix("UPDATE", "UPDATE")}`,
      `F.watchers: validate[0] ${fix("CONNECT, DISCONNECT", "CREATE_RELATIONSHIP, DELETE_RELATIONSHIP", "")}`,
    ]);
    // A claim beside the rule, the other fix the message names.
    const guarded = openOrOwned(off)
      .replace(
        "{ operations: [CREATE], where: { node: { open: { eq: true } } } }",
        "{ operations: [CREATE], where: { jwt: { sub: { exists: true } }, node: { open: { eq: true } } } }",
      )
      .replace(
        "where: { source: { open: { eq: true } } }",
        "where: { jwt: { sub: { exists: true } }, source: { open: { eq: true } } }",
      );
    expect(await signedOut(guarded)).toEqual([
      `F: validate[1] ${fix("UPDATE", "UPDATE")}`,
    ]);
    // An operation the type lists as public was opened on purpose.
    const publicType = `${off}type Claims @jwt { sub: String! }
type G @node @mutation(operations: [CREATE, UPDATE])
  @authorization(public: [CREATE], validate: [{ operations: [CREATE, UPDATE], where: { node: { open: { eq: true } } } }]) {
  key: String! @key
  open: Boolean!
}`;
    expect(await signedOut(publicType)).toEqual([
      `G: validate[0] ${fix("UPDATE", "UPDATE", " to G")}`,
    ]);
    // With the default true or unset, no rule is open to them.
    for (const extra of [
      "",
      "extend schema @authorizationDefaults(requireAuthentication: true)\n",
    ]) {
      expect(await signedOut(openOrOwned(extra))).toEqual([]);
    }
  });

  test("a field rule the bypass skips", async () => {
    const found = await lint(
      claims +
        `extend schema @authorizationDefaults(bypass: { jwt: { roles: { includes: "admin" } } })
type Doc @node { key: String! @key
  secret: String @authorization(validate: [{ operations: [READ], where: { jwt: { roles: { includes: "auditor" } } } }]) }`,
    );
    expect(found).toContain(
      "Doc.secret: the schema's bypass skips the rules of Doc.secret for callers passing it; add @authorization(bypass: false) to Doc if they must hold for everyone, or bypass: true to acknowledge it",
    );
  });

  test("every type the bypass reaches, in one line", async () => {
    const typeDefs = (vault: string) =>
      claims +
      `extend schema @authorizationDefaults(bypass: { jwt: { roles: { includes: "admin" } } })
type Open @node { key: String! @key }
type Doc @node @authorization(filter: [{ where: { node: { owner: { eq: "$jwt.sub" } } } }]) {
  key: String! @key  owner: String!
  links: [Doc!]! @relationship(type: "LINKS", direction: OUT, properties: "Link")
}
type Link @relationshipProperties {
  note: String @authorization(validate: [{ operations: [READ], where: { jwt: { roles: { includes: "auditor" } } } }])
}
type Masked @node { key: String! @key
  secret: String @authorization(mask: [{ unless: { jwt: { roles: { includes: "auditor" } } } }]) }
type Vault @node @authorization(${vault}filter: [{ where: { node: { owner: { eq: "$jwt.sub" } } } }]) {
  key: String! @key  owner: String! }`;
    const reach = (found: string[]) =>
      found.filter((l) =>
        l.includes("@authorizationDefaults(bypass:) reaches"),
      );
    // Masked acknowledges it and Open has no rules: neither is listed.
    expect(reach(await lint(typeDefs("")))).toEqual([
      "schema: @authorizationDefaults(bypass:) reaches the rules of 3 types that say neither bypass: true nor bypass: false (Doc, Masked, Vault), and the property rules of Link, which cannot opt out; callers passing it skip them",
    ]);
    expect(
      reach(
        await lint(
          typeDefs("bypass: false, ").replace(
            "type Masked @node {",
            "type Masked @node @authorization(bypass: true) {",
          ),
        ),
      ),
    ).toEqual([
      "schema: @authorizationDefaults(bypass:) reaches the rules of 1 type that says neither bypass: true nor bypass: false (Doc), and the property rules of Link, which cannot opt out; callers passing it skip them",
    ]);
  });

  test("bypass: true acknowledges the bypass", async () => {
    const typeDefs = (bypass: string) =>
      claims +
      `extend schema @authorizationDefaults(bypass: { jwt: { roles: { includes: "admin" } } })
type Doc @node ${bypass} { key: String! @key
  secret: String @authorization(validate: [{ operations: [READ], where: { jwt: { roles: { includes: "auditor" } } } }]) }`;
    const noted = (found: string[]) =>
      found.some((l) => l.includes("the schema's bypass skips"));
    expect(noted(await lint(typeDefs("")))).toBe(true);
    expect(noted(await lint(typeDefs("@authorization(bypass: true)")))).toBe(
      false,
    );
    // Still skipped for a caller passing the bypass.
    const t = await createTestLoraGraphQL({
      typeDefs: typeDefs("@authorization(bypass: true)"),
    });
    await t.db.execute("CREATE (:Doc {key: 'd', secret: 's'})");
    const admin = { jwt: { sub: "a", roles: ["admin"] } };
    expect(await t.data("{ docs { secret } }", {}, admin)).toEqual({
      docs: [{ secret: "s" }],
    });
    const other = await t.run("{ docs { secret } }", {}, { jwt: { sub: "b" } });
    expect(other.errors?.[0]?.extensions?.["code"]).toBe("FORBIDDEN");
  });

  const viewer = `type Claims @jwt { sub: String! @viewer(type: "Person", field: "key")  roles: [String!] }
type Person @node { key: String! @key }
`;

  test("a rule over a relationship the same operation re-points", async () => {
    const typeDefs = (trip: string, addedBy: string) =>
      viewer +
      `type Trip @node
  @authorizationRule(name: "crew", where: { node: { crew: { some: { isViewer: true } } } }) {
  key: String! @key
  crew: [Person!]! @relationship(type: "CREW", direction: IN)
}
type TripStop @node @mutation
  @authorization(validate: [
    { operations: [UPDATE, DELETE], where: { node: { trip: { rule: "crew" } } } }
    { operations: [CREATE], where: { AND: [{ node: { trip: { rule: "crew" } } }, { node: { addedBy: { isViewer: true } } }] } }
  ]) {
  key: String! @key(generate: true)
  trip: Trip! @relationship(type: "STOP", direction: IN, nestedOperations: [CONNECT]) ${trip}
  addedBy: Person! @relationship(type: "ADDED", direction: IN, nestedOperations: [CONNECT]) ${addedBy}
}`;
    const found = await lint(typeDefs("", ""));
    expect(found).toContain(
      "TripStop.trip: validate[0] guards UPDATE by testing trip, which the update input can re-point (connect): the rule runs before and after the write, so a caller who passes it on both the old and the new Trip moves the TripStop; declare trip @settable(onCreate: true, onUpdate: false) if it is fixed once created",
    );
    expect(found).toContain(
      "TripStop.addedBy: validate[1] tests addedBy on CREATE, but UPDATE can re-point it and no UPDATE rule tests it: a caller can create the TripStop as the rule demands, then change addedBy; declare addedBy @settable(onCreate: true, onUpdate: false), or add an UPDATE rule that tests it",
    );
    const fixed = "@settable(onCreate: true, onUpdate: false)";
    expect(
      (await lint(typeDefs(fixed, fixed))).filter((l) =>
        l.startsWith("TripStop."),
      ),
    ).toEqual([]);
  });

  test("a rule that reads a relationship through ${node.…} or <field>Exists", async () => {
    const typeDefs = (to: string, room: string) =>
      viewer +
      `type Room @node {
  key: String! @key
  people: [Person!]! @relationship(type: "IN", direction: IN)
}
type Invite @node @mutation
  @authorization(validate: [
    { operations: [UPDATE], where: { AND: [
      { node: { room: { people: { some: { key: { eq: "\${node.to.key}" } } } } } }
      { node: { roomExists: true } }
    ] } }
  ]) {
  key: String! @key(generate: true)
  to: Person! @relationship(type: "TO", direction: OUT, nestedOperations: [CONNECT]) ${to}
  room: Room @relationship(type: "FOR", direction: OUT, nestedOperations: [CONNECT]) ${room}
}`;
    const found = (await lint(typeDefs("", ""))).filter((l) =>
      l.startsWith("Invite."),
    );
    expect(found.some((l) => l.startsWith("Invite.to: validate[0]"))).toBe(
      true,
    );
    expect(found.some((l) => l.startsWith("Invite.room: validate[0]"))).toBe(
      true,
    );
    const fixed = "@settable(onCreate: true, onUpdate: false)";
    expect(
      (await lint(typeDefs(fixed, fixed))).filter((l) =>
        l.startsWith("Invite."),
      ),
    ).toEqual([]);
  });

  test("a rule pinned to the caller re-points only to them", async () => {
    const found = await lint(
      viewer +
        `type Note @node @mutation
  @authorization(validate: [{ operations: [CREATE, UPDATE, DELETE], where: { node: { owner: { isViewer: true } } } }]) {
  key: String! @key(generate: true)
  owner: Person! @relationship(type: "OWNS", direction: IN)
}`,
    );
    expect(found.filter((l) => l.startsWith("Note."))).toEqual([]);
  });

  test("a scalar a CREATE rule tests is state an update may move on", async () => {
    // Created as a DRAFT, then published by its owner: not a finding.
    const found = await lint(
      claims +
        `type Doc @node @mutation
  @authorization(validate: [
    { operations: [CREATE], where: { node: { status: { eq: DRAFT } } } }
    { operations: [UPDATE, DELETE], where: { node: { owner: { eq: "$jwt.sub" } } } }
  ]) {
  key: String! @key(generate: true)
  owner: String!
  status: Status!
}
enum Status { DRAFT PUBLISHED }`,
    );
    expect(found.some((l) => l.startsWith("Doc."))).toBe(false);
  });

  test("a type only an admin updates has nothing to re-point", async () => {
    const found = await lint(
      viewer +
        `type Room @node { key: String! @key }
type Message @node @mutation
  @authorization(
    filter: [{ operations: [READ, UPDATE], where: { node: { room: { key: { eq: "x" } } } } }]
    validate: [
      { operations: [CREATE], where: { node: { author: { isViewer: true } } } }
      { operations: [UPDATE], where: { jwt: { roles: { includes: "admin" } } } }
    ]
  ) {
  key: String! @key(generate: true)
  room: Room! @relationship(type: "IN", direction: OUT, nestedOperations: [CONNECT])
  author: Person! @relationship(type: "SENT", direction: IN, nestedOperations: [CONNECT])
}`,
    );
    expect(found.filter((l) => l.startsWith("Message."))).toEqual([]);
  });

  test("a validate rule over a masked field", async () => {
    const typeDefs = (rule: string) =>
      viewer +
      `type Request @node @mutation
  @authorization(validate: [
    { operations: [CREATE], where: { node: { from: { isViewer: true } } } }
    { operations: [UPDATE], where: { node: { to: { isViewer: true } } } }
    { operations: [DELETE], where: ${rule} }
  ]) {
  key: String! @key(generate: true)
  status: Status! @default(value: PENDING) @settable(onCreate: false, onUpdate: true)
    @authorization(mask: [{ unless: { OR: [{ node: { status: { in: [PENDING, ACCEPTED] } } }, { node: { to: { isViewer: true } } }] }, value: PENDING }])
  from: Person! @relationship(type: "SENT", direction: IN, nestedOperations: [CONNECT]) @settable(onCreate: true, onUpdate: false)
  to: Person! @relationship(type: "RECEIVED", direction: OUT, nestedOperations: [CONNECT]) @settable(onCreate: true, onUpdate: false)
}
enum Status { PENDING ACCEPTED DECLINED }`;
    const found = await lint(
      typeDefs(
        `{ AND: [{ node: { from: { isViewer: true } } }, { node: { status: { eq: PENDING } } }] }`,
      ),
    );
    expect(found).toContain(
      "Request.status: validate[2] (DELETE) tests Request.status, which a mask hides from callers failing its unless; rules see the stored value, so whether the request succeeds or is FORBIDDEN tells such a caller the hidden value. Test what the caller can see, or make sure every caller the rule decides for passes the mask",
    );
    expect(
      (await lint(typeDefs(`{ node: { from: { isViewer: true } } }`))).some(
        (l) => l.includes("a mask hides"),
      ),
    ).toBe(false);
  });

  test("a relationship rule over a masked field of the target", async () => {
    const typeDefs = (mask: string) =>
      viewer.replace(
        "type Person @node { key: String! @key }",
        `type Person @node { key: String! @key  hidden: Boolean! @authorization(mask: [{ unless: ${mask}, value: false }]) }`,
      ) +
      `type Group @node @mutation @authorization(public: [CREATE, UPDATE, DELETE]) {
  key: String! @key
  members: [Person!]! @relationship(type: "IN", direction: IN, nestedOperations: [CONNECT])
    @authorization(validate: [{ operations: [CONNECT], where: { target: { hidden: { eq: false } } } }])
}`;
    expect(await lint(typeDefs(`{ node: { isViewer: true } }`))).toContain(
      "Person.hidden: Group.members validate[0] (CONNECT) tests Person.hidden, which a mask hides from callers failing its unless; rules see the stored value, so whether the request succeeds or is FORBIDDEN tells such a caller the hidden value. Test what the caller can see, or make sure every caller the rule decides for passes the mask",
    );
    // Claims that settle the mask for every signed-in caller hide nothing.
    expect(
      (await lint(typeDefs(`{ jwt: { sub: { exists: true } } }`))).some((l) =>
        l.includes("a mask hides"),
      ),
    ).toBe(false);
  });

  test("a refused nested update keeps the edge through UPDATE_EDGE", async () => {
    const typeDefs = (ops: string) =>
      `type Claims @jwt { sub: String! @viewer(type: "Person", field: "key")  roles: [String!] }
type Festival @node @mutation
  @authorization(validate: [{ operations: [CREATE, UPDATE, DELETE], where: { jwt: { roles: { includes: "admin" } } } }]) {
  key: String! @key
  name: String!
}
type Attendance @relationshipProperties { status: String }
type Person @node @mutation
  @authorization(validate: [{ operations: [CREATE, UPDATE], where: { node: { isViewer: true } } }]) {
  key: String! @key
  festivals: [Festival!]!
    @relationship(type: "ATTENDS", direction: OUT, properties: "Attendance"${ops})
}`;
    const found = (await lint(typeDefs(""))).filter((l) =>
      l.startsWith("Person.festivals"),
    );
    expect(found.join("\n")).toMatch(
      /declare nestedOperations: \[CONNECT, DISCONNECT, UPDATE_EDGE\] on festivals/,
    );
    expect(
      (
        await lint(
          typeDefs(", nestedOperations: [CONNECT, DISCONNECT, UPDATE_EDGE]"),
        )
      ).filter((l) => l.startsWith("Person.festivals: nested")),
    ).toEqual([]);
  });

  test("nested writes the target's rules always refuse", async () => {
    const typeDefs = (nested: string) =>
      claims +
      `extend schema @authorizationDefaults(mutations: { jwt: { roles: { includes: "admin" } } })
type City @node @mutation { key: String! @key  name: String! }
type Festival @node @mutation
  @authorization(validate: [{ operations: [CREATE, UPDATE, DELETE], where: { node: { owner: { eq: "$jwt.sub" } } } }]) {
  key: String! @key
  owner: String! @settable(onCreate: true, onUpdate: false)
  city: City @relationship(type: "IN", direction: OUT${nested})
}`;
    expect(await lint(typeDefs(""))).toContain(
      "Festival.city: nested create, update, delete into City: its CREATE, UPDATE, DELETE rules refuse every signed-in caller without a role or claim beyond sub, so the input advertises writes only an admin (or the bypass) can make; declare nestedOperations: [CONNECT, DISCONNECT] on city",
    );
    expect(
      (await lint(typeDefs(", nestedOperations: [CONNECT, DISCONNECT]"))).some(
        (l) => l.startsWith("Festival.city"),
      ),
    ).toBe(false);
  });
});
