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
      "Doc.secret: the schema's bypass skips this field's rules for callers passing it; add @authorization(bypass: false) to the type if they must hold for everyone",
    );
  });
});
