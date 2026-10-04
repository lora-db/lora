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
      `type Doc @node @authorization(filter: [{ ${extra}${rule} }]) { key: String! @key  published: Boolean!  owner: String! }`;
    expect(await lint(typeDefs(""))).toContain(
      "Doc: filter[0] has a branch that needs no claims (OR[0]), but requireAuthentication defaults to true, so anonymous callers are refused by the whole rule; set requireAuthentication: false if anonymous callers should get that branch",
    );
    expect(
      (await lint(typeDefs("requireAuthentication: true, "))).some((l) =>
        l.includes("requireAuthentication defaults"),
      ),
    ).toBe(false);
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
